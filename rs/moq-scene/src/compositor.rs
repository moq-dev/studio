//! Draw scenes onto the canvas on the GPU: a preview, a program, and the fade
//! between them.

use std::collections::HashMap;
use std::time::Duration;

use kio::clock::Instant;
use moq_video::render::{Renderer, wgpu};

use crate::Error;
use crate::collection::{Canvas, Collection, Item, Layer as Place, Source};
use crate::effect::{self, Effects, Frame, Lut, Picture, Prepared};
use crate::source::{self, Sources};

/// The format of every canvas texture: gamma-encoded RGBA, what
/// `moq_video::render` produces and what its exporter hands an encoder.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Draws a [`Collection`]'s scenes onto its canvas: one texture for the scene
/// in preview, one for the scene on program, and the fade from one to the other.
///
/// Both textures are plain [`wgpu::Texture`]s on the caller's device, so a UI
/// shows them without a copy and an encoder reads the program from the same
/// memory. [`render`](Self::render) redraws both from the newest picture each
/// source has, so a source that stalls repeats its last picture rather than
/// holding up the canvas.
///
/// Pixels are blended in the gamma-encoded space sources arrive in, as a
/// broadcast switcher and CSS do, so a 50% fade is a 50% mix of the stored
/// values.
pub struct Compositor {
	device: wgpu::Device,
	queue: wgpu::Queue,
	canvas: Canvas,
	effects: Effects,

	preview: Target,
	program: Target,
	/// The scene fading in, drawn whole so it blends over the program as one
	/// picture rather than layer by layer.
	incoming: Target,

	layers: HashMap<String, Layer>,
	preview_scene: Option<String>,
	program_scene: Option<String>,
	fade: Option<Fade>,
}

/// A fade from `from` to the program scene.
struct Fade {
	from: Option<String>,
	start: Instant,
	duration: Duration,
}

/// A canvas-sized texture drawn into, and bound to sample.
struct Target {
	texture: wgpu::Texture,
	picture: Picture,
}

/// One source's newest picture or table on the GPU.
struct Layer {
	/// What the picture came from. An edited source starts a new layer, so it
	/// never shows the old source's last picture.
	source: Source,
	/// Converts video frames, created on the first one.
	renderer: Option<Renderer>,
	/// The newest picture; `None` until the first arrives.
	picture: Option<Picture>,
	/// A LUT source's table; `None` until it is read.
	lut: Option<Lut>,
	/// Why the newest picture failed to draw, logged when it first does.
	error: Option<String>,
}

/// The pictures and tables of the compositor's layers, for filters to read.
struct Library<'a>(&'a HashMap<String, Layer>);

impl effect::Library for Library<'_> {
	fn picture(&self, name: &str) -> Option<Picture> {
		self.0.get(name)?.picture.clone()
	}

	fn lut(&self, name: &str) -> Option<Lut> {
		self.0.get(name)?.lut.clone()
	}
}

impl Compositor {
	/// A compositor drawing `canvas`-sized pictures on `device`.
	///
	/// Fails if the device cannot hold a texture that large.
	pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, canvas: Canvas) -> Result<Self, Error> {
		let limit = device.limits().max_texture_dimension_2d;
		if canvas.width > limit || canvas.height > limit {
			return Err(Error::Invalid(format!(
				"canvas {}x{} exceeds this GPU's {limit}px texture limit",
				canvas.width, canvas.height
			)));
		}

		let effects = Effects::new(device, queue, canvas);
		let target = |label| Target::new(device, &effects, canvas, label);
		Ok(Self {
			preview: target("moq-scene preview"),
			program: target("moq-scene program"),
			incoming: target("moq-scene incoming"),
			device: device.clone(),
			queue: queue.clone(),
			canvas,
			effects,
			layers: HashMap::new(),
			preview_scene: None,
			program_scene: None,
			fade: None,
		})
	}

	/// The canvas this compositor draws.
	pub fn canvas(&self) -> Canvas {
		self.canvas
	}

	/// The scene in preview, as of the last [`render`](Self::render).
	pub fn preview(&self) -> &wgpu::Texture {
		&self.preview.texture
	}

	/// The program output, as of the last [`render`](Self::render). Created
	/// with `COPY_SRC | TEXTURE_BINDING`, what
	/// `moq_video::render::export::Exporter::copy` takes to hand it to an encoder.
	pub fn program(&self) -> &wgpu::Texture {
		&self.program.texture
	}

	/// The scene in preview.
	pub fn preview_scene(&self) -> Option<&str> {
		self.preview_scene.as_deref()
	}

	/// The scene on program, or the one fading in.
	pub fn program_scene(&self) -> Option<&str> {
		self.program_scene.as_deref()
	}

	/// Why `source`'s newest picture could not be drawn, until one is.
	pub fn error(&self, source: &str) -> Option<&str> {
		self.layers.get(source)?.error.as_deref()
	}

	/// Whether a fade is under way.
	pub fn fading(&self) -> bool {
		self.fade.is_some()
	}

	/// Put the scene called `scene` in preview, or nothing.
	pub fn set_preview(&mut self, scene: Option<String>) {
		self.preview_scene = scene;
	}

	/// Put the preview on program at once, ending any fade.
	pub fn cut(&mut self) {
		self.fade = None;
		self.program_scene = self.preview_scene.clone();
	}

	/// Fade the preview onto program over `duration`, starting at `now`.
	///
	/// A fade already under way lands on its scene first, so this one starts
	/// from what that fade was heading to.
	pub fn fade(&mut self, duration: Duration, now: Instant) {
		let from = std::mem::replace(&mut self.program_scene, self.preview_scene.clone());
		self.fade = (!duration.is_zero()).then_some(Fade {
			from,
			start: now,
			duration,
		});
	}

	/// Draw preview and program as of `now`, from the newest picture each of
	/// `sources` has.
	///
	/// A source whose picture fails to draw keeps its last one and reports why
	/// through [`error`](Self::error); this fails only when the canvas itself
	/// cannot be drawn.
	pub fn render(&mut self, collection: &Collection, sources: &Sources, now: Instant) -> Result<(), Error> {
		self.update(collection, sources);

		let progress = self.fade.as_ref().map(|fade| {
			let elapsed = now.saturating_duration_since(fade.start);
			(elapsed.as_secs_f32() / fade.duration.as_secs_f32()).min(1.0)
		});
		if progress.is_some_and(|progress| progress >= 1.0) {
			self.fade = None;
		}

		let mut frame = self.effects.frame();
		let mut draw = Draw {
			collection,
			effects: &mut self.effects,
			library: Library(&self.layers),
			frame: &mut frame,
			prepared: HashMap::new(),
		};

		// The top layer goes over both outputs, and over a fade rather than
		// through it, so it stays on across the switch.
		draw.target(&self.preview);
		draw.scene(&self.preview, self.preview_scene.as_deref());
		draw.layer(&self.preview, Place::Top);

		match (&self.fade, progress) {
			(Some(fade), Some(progress)) => {
				draw.target(&self.incoming);
				draw.scene(&self.incoming, self.program_scene.as_deref());
				draw.target(&self.program);
				draw.scene(&self.program, fade.from.as_deref());
				let incoming = draw.effects.fade(&self.incoming.picture, progress);
				draw.effects.draw(draw.frame, &self.program.texture, &incoming);
			}
			_ => {
				draw.target(&self.program);
				draw.scene(&self.program, self.program_scene.as_deref());
			}
		}
		draw.layer(&self.program, Place::Top);

		self.effects.submit(frame);
		Ok(())
	}

	/// Pull the newest picture from every source onto the GPU, and forget the
	/// sources the collection no longer has or has since edited.
	fn update(&mut self, collection: &Collection, sources: &Sources) {
		self.layers
			.retain(|name, layer| collection.sources.get(name) == Some(&layer.source));

		for (name, source) in &collection.sources {
			let Some(feed) = sources.get(name).filter(|feed| feed.source() == source) else {
				continue;
			};
			let Some(picture) = feed.take() else {
				continue;
			};
			let layer = self.layers.entry(name.clone()).or_insert_with(|| Layer {
				source: source.clone(),
				renderer: None,
				picture: None,
				lut: None,
				error: None,
			});
			// A page arrives premultiplied; frames and images are straight.
			let premultiplied = matches!(picture, source::Picture::Texture(_));
			let texture = match picture {
				source::Picture::Frame(frame) => {
					let renderer = match layer.renderer.as_mut() {
						Some(renderer) => Ok(renderer),
						None => Renderer::new(&self.device, &self.queue, Default::default())
							.map(|renderer| layer.renderer.insert(renderer)),
					};
					renderer
						.and_then(|renderer| renderer.render(&frame))
						.map_err(Error::from)
				}
				source::Picture::Rgba { size, pixels } => upload(&self.device, &self.queue, size, &pixels),
				source::Picture::Texture(texture) => Ok(texture),
				source::Picture::Lut(cube) => {
					layer.lut = Some(self.effects.lut(&cube));
					continue;
				}
			};

			match texture {
				Ok(texture) => {
					layer.error = None;
					layer.picture = Some(self.effects.picture(&texture, premultiplied));
				}
				Err(err) => {
					if layer.error.is_none() {
						tracing::warn!(source = %name, %err, "cannot draw source");
					}
					layer.error = Some(err.to_string());
				}
			}
		}
	}
}

/// One render's draws onto its targets.
struct Draw<'a> {
	collection: &'a Collection,
	effects: &'a mut Effects,
	library: Library<'a>,
	frame: &'a mut Frame,
	/// Each item, filtered once though it is drawn onto several targets.
	prepared: HashMap<(Place, usize), Option<Prepared>>,
}

impl Draw<'_> {
	/// Start drawing `target`.
	fn target(&mut self, target: &Target) {
		self.frame.target(&target.picture);
	}

	/// Draw `scene`'s items onto `target`. A missing scene draws nothing.
	fn scene(&mut self, target: &Target, scene: Option<&str>) {
		if let Some(scene) = scene.filter(|name| self.collection.scene(name).is_some()) {
			self.layer(target, Place::Scene(scene.to_string()));
		}
	}

	/// Draw `place`'s items onto `target`, back to front, each through its crop,
	/// filters, and blend mode. A hidden item, a source with no picture yet, or
	/// a filter whose source has nothing yet draws nothing.
	fn layer(&mut self, target: &Target, place: Place) {
		let items = self.collection.items(&place).map_or(&[][..], Vec::as_slice);
		for (index, item) in items.iter().enumerate().filter(|(_, item)| item.visible) {
			let key = (place.clone(), index);
			let prepared = match self.prepared.get(&key) {
				Some(prepared) => prepared.clone(),
				None => {
					let prepared = self.prepare(item);
					self.prepared.insert(key, prepared.clone());
					prepared
				}
			};
			if let Some(prepared) = prepared {
				self.effects.draw(self.frame, &target.texture, &prepared);
			}
		}
	}

	fn prepare(&mut self, item: &Item) -> Option<Prepared> {
		let collection = self.collection;
		let picture = self.library.0.get(collection.capture(&item.source))?.picture.as_ref()?;
		let item = effect::Item {
			picture,
			rect: item.rect,
			crop: collection.crop(item),
			filters: collection.filters(item).collect(),
			blend: item.blend,
		};
		self.effects.prepare(self.frame, item, &self.library)
	}
}

/// A texture holding tightly packed RGBA pixels.
fn upload(
	device: &wgpu::Device,
	queue: &wgpu::Queue,
	size: moq_video::Size,
	pixels: &[u8],
) -> Result<wgpu::Texture, Error> {
	let limit = device.limits().max_texture_dimension_2d;
	if size.width == 0 || size.height == 0 || size.width > limit || size.height > limit {
		return Err(Error::Invalid(format!(
			"a {size} picture does not fit this GPU's {limit}px texture limit"
		)));
	}

	let extent = wgpu::Extent3d {
		width: size.width,
		height: size.height,
		depth_or_array_layers: 1,
	};
	let texture = device.create_texture(&wgpu::TextureDescriptor {
		label: Some("moq-scene picture"),
		size: extent,
		mip_level_count: 1,
		sample_count: 1,
		dimension: wgpu::TextureDimension::D2,
		format: FORMAT,
		usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
		view_formats: &[],
	});
	queue.write_texture(
		wgpu::TexelCopyTextureInfo {
			texture: &texture,
			mip_level: 0,
			origin: wgpu::Origin3d::ZERO,
			aspect: wgpu::TextureAspect::All,
		},
		pixels,
		wgpu::TexelCopyBufferLayout {
			offset: 0,
			bytes_per_row: Some(size.width * 4),
			rows_per_image: Some(size.height),
		},
		extent,
	);
	Ok(texture)
}

impl Target {
	fn new(device: &wgpu::Device, effects: &Effects, canvas: Canvas, label: &str) -> Self {
		let texture = device.create_texture(&wgpu::TextureDescriptor {
			label: Some(label),
			size: wgpu::Extent3d {
				width: canvas.width,
				height: canvas.height,
				depth_or_array_layers: 1,
			},
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: FORMAT,
			// A blend mode copies what is under it out of the target.
			usage: wgpu::TextureUsages::RENDER_ATTACHMENT
				| wgpu::TextureUsages::TEXTURE_BINDING
				| wgpu::TextureUsages::COPY_SRC,
			view_formats: &[],
		});
		let picture = effects.picture(&texture, false);
		Self { texture, picture }
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::collection::{Blend, Filter, MaskMode, Rect, Rgba, Scene};
	use crate::source::test::{context, local, start};

	const RED: [u8; 4] = [255, 0, 0, 255];
	const BLUE: [u8; 4] = [0, 0, 255, 255];
	const BLACK: [u8; 4] = [0, 0, 0, 255];

	async fn gpu() -> (wgpu::Device, wgpu::Queue) {
		let instance = wgpu::Instance::default();
		let adapter = instance
			.request_adapter(&wgpu::RequestAdapterOptions::default())
			.await
			.expect("a GPU adapter");
		adapter
			.request_device(&wgpu::DeviceDescriptor::default())
			.await
			.expect("a GPU device")
	}

	/// An 8x8 canvas with a red and a blue source, and scenes "red" (red filling
	/// the canvas, blue over its top-right quarter) and "blue" (blue filling it).
	fn collection() -> Collection {
		let mut collection = Collection::new();
		collection.canvas = Canvas {
			width: 8,
			height: 8,
			framerate: 30,
		};
		collection
			.sources
			.insert("red".into(), Source::Color { color: Rgba(RED) });
		collection
			.sources
			.insert("blue".into(), Source::Color { color: Rgba(BLUE) });
		let item = Item::new;
		let fill = Rect::fill(&collection.canvas);
		let corner = Rect {
			x: 4.0,
			y: 0.0,
			width: 4.0,
			height: 4.0,
		};
		collection.scenes = vec![
			Scene {
				name: "red".into(),
				items: vec![item("red", fill), item("blue", corner)],
			},
			Scene {
				name: "blue".into(),
				items: vec![item("blue", fill)],
			},
		];
		collection.validate().unwrap();
		collection
	}

	/// A compositor over running sources, read back to check pixels.
	struct Studio {
		collection: Collection,
		sources: Sources,
		compositor: Compositor,
		device: wgpu::Device,
		queue: wgpu::Queue,
		_origin: moq_net::origin::Producer,
		_dir: tempfile::TempDir,
	}

	impl Studio {
		async fn new(collection: Collection) -> Self {
			Self::with_files(collection, tempfile::tempdir().unwrap()).await
		}

		/// Over sources whose files are in `dir`.
		async fn with_files(collection: Collection, dir: tempfile::TempDir) -> Self {
			let (device, queue) = gpu().await;
			let (mut context, origin) = context(dir.path());
			context.output = moq_video::render::output(&device);
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			Self {
				compositor: Compositor::new(&device, &queue, collection.canvas).unwrap(),
				device,
				queue,
				collection,
				sources,
				_origin: origin,
				_dir: dir,
			}
		}

		fn render(&mut self, now: Instant) {
			self.compositor.render(&self.collection, &self.sources, now).unwrap();
		}

		/// The canvas texture's exact RGBA rows: no 4:2:0 round trip, so a test
		/// compares the stored values.
		fn read(&mut self, texture: &wgpu::Texture) -> Vec<u8> {
			let canvas = self.compositor.canvas();
			let row = (canvas.width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
			let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
				label: None,
				size: u64::from(row * canvas.height),
				usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
				mapped_at_creation: false,
			});
			let mut encoder = self.device.create_command_encoder(&Default::default());
			encoder.copy_texture_to_buffer(
				texture.as_image_copy(),
				wgpu::TexelCopyBufferInfo {
					buffer: &buffer,
					layout: wgpu::TexelCopyBufferLayout {
						offset: 0,
						bytes_per_row: Some(row),
						rows_per_image: None,
					},
				},
				texture.size(),
			);
			self.queue.submit([encoder.finish()]);
			buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
			self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
			let width = canvas.width as usize * 4;
			let mapped = buffer.slice(..).get_mapped_range().unwrap();
			mapped
				.chunks_exact(row as usize)
				.flat_map(|row| row[..width].to_vec())
				.collect()
		}

		fn program(&mut self) -> Vec<u8> {
			let texture = self.compositor.program().clone();
			self.read(&texture)
		}

		fn preview(&mut self) -> Vec<u8> {
			let texture = self.compositor.preview().clone();
			self.read(&texture)
		}
	}

	fn pixel(rgba: &[u8], x: usize, y: usize) -> [u8; 4] {
		let at = (y * 8 + x) * 4;
		rgba[at..at + 4].try_into().unwrap()
	}

	fn near(actual: [u8; 4], expected: [u8; 4]) -> bool {
		actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 1)
	}

	/// Items draw where their rect says, later items over earlier ones, and
	/// a hidden item not at all.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn items_draw_in_place_and_in_order() {
		local(async {
			let mut studio = Studio::new(collection()).await;
			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			studio.render(Instant::now());
			let program = studio.program();
			assert_eq!(pixel(&program, 1, 1), RED);
			assert_eq!(pixel(&program, 6, 1), BLUE);
			assert_eq!(pixel(&program, 6, 6), RED);
			assert_eq!(studio.preview(), program, "preview shows the same scene");

			studio.collection.scenes[0].items[1].visible = false;
			studio.render(Instant::now());
			assert_eq!(pixel(&studio.program(), 6, 1), RED);
		})
		.await;
	}

	/// Straight alpha blends over what is below, and the canvas stays opaque.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn alpha_blends_over_black() {
		local(async {
			let mut collection = collection();
			collection.sources.insert(
				"red".into(),
				Source::Color {
					color: Rgba([255, 0, 0, 128]),
				},
			);
			collection.scenes[0].items.truncate(1);
			let mut studio = Studio::new(collection).await;
			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			studio.render(Instant::now());
			let actual = pixel(&studio.program(), 1, 1);
			assert!(near(actual, [128, 0, 0, 255]), "{actual:?}");
		})
		.await;
	}

	/// A fade mixes the outgoing and incoming scenes by its progress on the
	/// program clock, and lands on the incoming scene; preview never fades.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_fade_mixes_by_progress() {
		local(async {
			let mut studio = Studio::new(collection()).await;
			let start = Instant::now();
			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			studio.compositor.set_preview(Some("blue".into()));
			studio.compositor.fade(Duration::from_secs(1), start);
			assert_eq!(studio.compositor.program_scene(), Some("blue"));

			studio.render(start + Duration::from_millis(500));
			assert!(studio.compositor.fading());
			let program = studio.program();
			let actual = pixel(&program, 1, 1);
			assert!(near(actual, [128, 0, 127, 255]), "{actual:?}");
			assert_eq!(pixel(&program, 6, 1), BLUE, "blue under blue stays blue");
			assert_eq!(pixel(&studio.preview(), 1, 1), BLUE);

			studio.render(start + Duration::from_secs(1));
			assert!(!studio.compositor.fading());
			assert_eq!(pixel(&studio.program(), 1, 1), BLUE);
		})
		.await;
	}

	/// The bottom-left quarter of the 8x8 canvas.
	const CORNER: Rect = Rect {
		x: 0.0,
		y: 4.0,
		width: 4.0,
		height: 4.0,
	};

	/// A clone draws its source's picture through its own filters, then the
	/// item's, and leaves the source it clones as it was.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_clone_draws_through_its_own_filters() {
		local(async {
			let mut collection = collection();
			collection
				.sources
				.insert("blue faded".into(), Source::Clone { source: "blue".into() });
			collection.video.insert(
				"blue faded".into(),
				crate::Video {
					filters: vec![Filter::Opacity { amount: 0.5 }],
					..Default::default()
				},
			);
			collection.scenes[0].items.push(Item {
				filters: vec![Filter::Opacity { amount: 0.5 }],
				..Item::new("blue faded", CORNER)
			});
			collection.validate().unwrap();

			let mut studio = Studio::new(collection).await;
			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			studio.render(Instant::now());
			let program = studio.program();
			let actual = pixel(&program, 1, 6);
			assert!(near(actual, [191, 0, 64, 255]), "a quarter blue over red: {actual:?}");
			assert_eq!(pixel(&program, 6, 1), BLUE, "the source itself is unfiltered");
		})
		.await;
	}

	/// The top layer draws over preview and program, and over a fade rather
	/// than fading with it.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn the_top_layer_stays_over_a_fade() {
		local(async {
			let mut collection = collection();
			collection.top.push(Item::new("blue", CORNER));
			let mut studio = Studio::new(collection).await;
			let start = Instant::now();
			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			studio.render(start);
			assert_eq!(pixel(&studio.program(), 1, 6), BLUE);

			studio.compositor.set_preview(Some("blue".into()));
			studio.compositor.fade(Duration::from_secs(1), start);
			studio.render(start + Duration::from_millis(500));
			let program = studio.program();
			let actual = pixel(&program, 1, 1);
			assert!(near(actual, [128, 0, 127, 255]), "{actual:?}");
			assert_eq!(pixel(&program, 1, 6), BLUE, "the top layer does not fade");
			studio.collection.scenes[1].items.clear();
			studio.render(start + Duration::from_millis(600));
			assert_eq!(pixel(&studio.preview(), 1, 6), BLUE);
			assert_eq!(pixel(&studio.preview(), 6, 1), BLACK);
		})
		.await;
	}

	/// A cut lands at once, even mid-fade, and an empty program is black.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_cut_ends_a_fade() {
		local(async {
			let mut studio = Studio::new(collection()).await;
			let start = Instant::now();
			studio.render(start);
			assert_eq!(pixel(&studio.program(), 1, 1), BLACK);

			studio.compositor.set_preview(Some("blue".into()));
			studio.compositor.fade(Duration::from_secs(1), start);
			studio.render(start + Duration::from_millis(500));
			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			assert!(!studio.compositor.fading());
			studio.render(start + Duration::from_millis(600));
			assert_eq!(pixel(&studio.program(), 1, 1), RED);
		})
		.await;
	}

	/// A source with no new picture repeats its last one; an edited source never
	/// shows the old one's picture, and a removed source disappears.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_stalled_source_repeats_its_last_picture() {
		local(async {
			let mut studio = Studio::new(collection()).await;
			studio.compositor.set_preview(Some("blue".into()));
			studio.compositor.cut();
			studio.render(Instant::now());
			assert!(studio.sources.get("blue").unwrap().take().is_none(), "drawn once");
			studio.render(Instant::now());
			assert_eq!(pixel(&studio.program(), 1, 1), BLUE);

			// An offline broadcast has no picture yet, so it draws nothing.
			studio.collection.sources.insert(
				"blue".into(),
				Source::Broadcast {
					path: "room/offline".into(),
				},
			);
			let drivers = studio.sources.sync(&studio.collection);
			start(&studio.sources, drivers).await;
			studio.render(Instant::now());
			assert_eq!(pixel(&studio.program(), 1, 1), BLACK);

			studio.collection.sources.remove("blue");
			studio.collection.scenes.clear();
			studio.render(Instant::now());
			assert_eq!(pixel(&studio.program(), 1, 1), BLACK);
		})
		.await;
	}

	/// A picture the GPU cannot hold says why, rather than leaving its source
	/// live and missing from the canvas.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_picture_that_cannot_draw_reports_why() {
		local(async {
			let mut collection = collection();
			let mut studio = Studio::new(collection.clone()).await;
			let limit = studio.compositor.device.limits().max_texture_dimension_2d;
			image::RgbaImage::new(limit + 1, 1)
				.save(studio._dir.path().join("wide.png"))
				.unwrap();
			collection.sources.insert(
				"red".into(),
				Source::Image {
					path: studio._dir.path().join("wide.png"),
				},
			);
			let drivers = studio.sources.sync(&collection);
			start(&studio.sources, drivers).await;
			studio.collection = collection;
			studio.render(Instant::now());
			let err = studio.compositor.error("red").expect("the picture failed");
			assert!(err.contains("texture limit"), "{err}");
			assert!(studio.compositor.error("blue").is_none());
		})
		.await;
	}

	/// The program of `collection` with scene "red" cut to it, its files in `dir`.
	async fn drawn(collection: Collection, dir: tempfile::TempDir) -> Vec<u8> {
		let mut studio = Studio::with_files(collection, dir).await;
		studio.compositor.set_preview(Some("red".into()));
		studio.compositor.cut();
		studio.render(Instant::now());
		studio.program()
	}

	/// Scene "red" as `items` alone, over black.
	fn showing(items: Vec<Item>) -> Collection {
		let mut collection = collection();
		collection.scenes[0].items = items;
		collection.validate().unwrap();
		collection
	}

	fn rgb(rgb: [u8; 3]) -> [u8; 4] {
		[rgb[0], rgb[1], rgb[2], 255]
	}

	/// Each CSS color filter maps a color as the spec's matrix does, clamping
	/// after each filter rather than once at the end.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn color_filters_follow_css() {
		let cases = [
			(vec![Filter::Grayscale { amount: 1.0 }], [118, 118, 118]),
			(vec![Filter::Sepia { amount: 1.0 }], [165, 147, 114]),
			(vec![Filter::Invert { amount: 1.0 }], [55, 155, 205]),
			(vec![Filter::Brightness { amount: 0.5 }], [100, 50, 25]),
			(vec![Filter::Contrast { amount: 0.0 }], [128, 128, 128]),
			(vec![Filter::Saturate { amount: 0.0 }], [118, 118, 118]),
			(vec![Filter::HueRotate { degrees: 180.0 }], [35, 135, 185]),
			(vec![Filter::Opacity { amount: 0.5 }], [100, 50, 25]),
			(
				vec![Filter::Brightness { amount: 2.0 }, Filter::Brightness { amount: 0.5 }],
				[128, 100, 50],
			),
		];
		local(async {
			for (filters, expected) in cases {
				let mut collection = collection();
				collection.sources.insert(
					"tan".into(),
					Source::Color {
						color: Rgba([200, 100, 50, 255]),
					},
				);
				collection.scenes[0].items = vec![Item {
					filters: filters.clone(),
					..Item::new("tan", Rect::fill(&collection.canvas))
				}];
				let program = drawn(collection, tempfile::tempdir().unwrap()).await;
				let actual = pixel(&program, 3, 3);
				assert!(near(actual, rgb(expected)), "{filters:?}: {actual:?}");
			}
		})
		.await;
	}

	/// A crop trims the picture and stretches the rest over the item: an image
	/// red on its left half and blue on its right shows blue where red was.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_crop_fills_the_item() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			image::RgbaImage::from_fn(8, 8, |x, _| image::Rgba(if x < 4 { RED } else { BLUE }))
				.save(dir.path().join("halves.png"))
				.unwrap();
			let mut collection = collection();
			collection.sources.insert(
				"halves".into(),
				Source::Image {
					path: "halves.png".into(),
				},
			);
			let fill = Rect::fill(&collection.canvas);
			collection.scenes[0].items = vec![Item {
				crop: crate::Crop {
					left: 0.5,
					..Default::default()
				},
				..Item::new("halves", fill)
			}];
			let program = drawn(collection, dir).await;
			assert_eq!(pixel(&program, 3, 4), BLUE);
		})
		.await;
	}

	/// A blur is a Gaussian of the picture that fades into transparency past
	/// its edges, as CSS's edge mode none: a blue square bleeds onto the black
	/// around it by the discrete Gaussian's weights.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_blur_spreads_past_the_edges() {
		local(async {
			let square = Rect {
				x: 2.0,
				y: 2.0,
				width: 4.0,
				height: 4.0,
			};
			let program = drawn(
				showing(vec![Item {
					filters: vec![Filter::Blur { radius: 1.0 }],
					..Item::new("blue", square)
				}]),
				tempfile::tempdir().unwrap(),
			)
			.await;
			// The weights of offsets 0 to 3 at a standard deviation of 1.
			let weight = |offset: i32| (-(offset * offset) as f32 / 2.0).exp();
			let total: f32 = (-3..=3).map(weight).sum();
			let covered = |offsets: std::ops::RangeInclusive<i32>| offsets.map(weight).sum::<f32>() / total;
			// One pixel left of the square, on its second row.
			let expected = covered(1..=3) * covered(-1..=2) * 255.0;
			let actual = pixel(&program, 1, 3);
			assert!(
				near(actual, [0, 0, expected.round() as u8, 255]),
				"{actual:?} vs {expected}"
			);
			assert_eq!(pixel(&program, 1, 3), pixel(&program, 6, 4), "symmetric");
		})
		.await;
	}

	/// A blur many pixels wide is drawn on halved rasters, still matching the
	/// Gaussian of a square to within a few levels.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_wide_blur_matches_the_gaussian() {
		local(async {
			let mut collection = collection();
			collection.canvas = Canvas {
				width: 64,
				height: 64,
				framerate: 30,
			};
			collection.scenes[0].items = vec![Item {
				filters: vec![Filter::Blur { radius: 10.0 }],
				..Item::new("blue", Rect::fill(&collection.canvas))
			}];
			let program = drawn(collection, tempfile::tempdir().unwrap()).await;
			let at = |x: usize, y: usize| program[(y * 64 + x) * 4 + 2];
			// The share of a Gaussian of deviation 10, centered half a pixel
			// inside an edge, that falls inside: Φ(0.05).
			let edge = 0.52;
			let cases = [
				((32, 32), 255.0),
				((0, 32), edge * 255.0),
				((0, 0), edge * edge * 255.0),
			];
			for ((x, y), expected) in cases {
				let actual = f32::from(at(x, y));
				assert!((actual - expected).abs() <= 4.0, "({x}, {y}): {actual} vs {expected}");
			}
		})
		.await;
	}

	/// A drop shadow is the picture's shape, offset and filled, under it.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_drop_shadow_falls_under() {
		local(async {
			let square = Rect {
				x: 2.0,
				y: 2.0,
				width: 2.0,
				height: 2.0,
			};
			let program = drawn(
				showing(vec![Item {
					filters: vec![Filter::DropShadow {
						x: 1.0,
						y: 2.0,
						blur: 0.0,
						color: Rgba([255, 0, 0, 128]),
					}],
					..Item::new("blue", square)
				}]),
				tempfile::tempdir().unwrap(),
			)
			.await;
			assert_eq!(pixel(&program, 3, 3), BLUE, "the picture over its shadow");
			let actual = pixel(&program, 4, 5);
			assert!(near(actual, [128, 0, 0, 255]), "the shadow alone: {actual:?}");
			assert_eq!(pixel(&program, 4, 2), BLACK, "beside both");
		})
		.await;
	}

	/// A chroma key hides its color and keeps the rest; a luma key hides what
	/// is out of its range.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn keys_hide_what_they_key() {
		local(async {
			let mut collection = collection();
			let green = Rgba([0, 255, 0, 255]);
			for (name, color) in [("green", green), ("white", Rgba([255; 4]))] {
				collection.sources.insert(name.into(), Source::Color { color });
			}
			let chroma = Filter::ChromaKey {
				color: green,
				similarity: 0.4,
				smoothness: 0.08,
				spill: 0.1,
			};
			let luma = Filter::LumaKey {
				min: 0.0,
				max: 0.5,
				smoothness: 0.0,
			};
			let left = Rect {
				x: 0.0,
				y: 0.0,
				width: 4.0,
				height: 8.0,
			};
			let right = Rect { x: 4.0, ..left };
			let keyed = |source: &str, rect, filter: &Filter| Item {
				filters: vec![filter.clone()],
				..Item::new(source, rect)
			};
			collection.scenes[0].items = vec![
				Item::new("red", Rect::fill(&collection.canvas)),
				keyed("green", left, &chroma),
				keyed("blue", right, &chroma),
				keyed("white", Rect { height: 4.0, ..left }, &luma),
			];
			let program = drawn(collection, tempfile::tempdir().unwrap()).await;
			assert_eq!(pixel(&program, 1, 6), RED, "green is keyed out");
			assert_eq!(pixel(&program, 6, 6), BLUE, "blue is kept");
			assert_eq!(pixel(&program, 1, 1), RED, "white is too bright");
		})
		.await;
	}

	/// A LUT maps each color through its table, interpolating between points.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_lut_grades() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let mut cube = String::from("LUT_3D_SIZE 2\n");
			for b in 0..2 {
				for g in 0..2 {
					for r in 0..2 {
						cube += &format!("{} {} {}\n", 1 - r, 1 - g, 1 - b);
					}
				}
			}
			std::fs::write(dir.path().join("invert.cube"), cube).unwrap();
			let mut collection = collection();
			collection.sources.insert(
				"invert".into(),
				Source::Lut {
					path: "invert.cube".into(),
				},
			);
			collection.sources.insert(
				"tan".into(),
				Source::Color {
					color: Rgba([200, 100, 50, 255]),
				},
			);
			let lut = |amount| Filter::Lut {
				source: "invert".into(),
				amount,
			};
			let half = Rect {
				x: 0.0,
				y: 0.0,
				width: 8.0,
				height: 4.0,
			};
			collection.scenes[0].items = vec![
				Item {
					filters: vec![lut(1.0)],
					..Item::new("tan", half)
				},
				Item {
					filters: vec![lut(0.5)],
					..Item::new("blue", Rect { y: 4.0, ..half })
				},
			];
			let program = drawn(collection, dir).await;
			let actual = pixel(&program, 3, 1);
			assert!(near(actual, rgb([55, 155, 205])), "{actual:?}");
			let actual = pixel(&program, 3, 6);
			assert!(near(actual, rgb([128, 128, 128])), "half graded: {actual:?}");
		})
		.await;
	}

	/// A mask keeps the picture where its source is opaque, or bright in
	/// luminance mode, and an item whose mask has no picture yet draws nothing.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_mask_keeps_what_it_covers() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			image::RgbaImage::from_fn(8, 8, |x, _| {
				image::Rgba(if x < 4 { [255; 4] } else { [255, 255, 255, 0] })
			})
			.save(dir.path().join("alpha.png"))
			.unwrap();
			image::RgbaImage::from_fn(8, 8, |x, _| image::Rgba(if x < 4 { [255; 4] } else { BLACK }))
				.save(dir.path().join("luma.png"))
				.unwrap();
			let mut collection = collection();
			for (name, path) in [("alpha", "alpha.png"), ("luma", "luma.png")] {
				collection
					.sources
					.insert(name.into(), Source::Image { path: path.into() });
			}
			collection.sources.insert(
				"offline".into(),
				Source::Broadcast {
					path: "room/offline".into(),
				},
			);
			let masked = |mask: &str, mode, y| Item {
				filters: vec![Filter::Mask {
					source: mask.into(),
					mode,
				}],
				..Item::new(
					"blue",
					Rect {
						x: 0.0,
						y,
						width: 8.0,
						height: 2.0,
					},
				)
			};
			collection.scenes[0].items = vec![
				Item::new("red", Rect::fill(&collection.canvas)),
				masked("alpha", MaskMode::Alpha, 0.0),
				masked("luma", MaskMode::Luminance, 2.0),
				masked("offline", MaskMode::Alpha, 4.0),
			];
			let program = drawn(collection, dir).await;
			for y in [0, 2] {
				assert_eq!(pixel(&program, 1, y), BLUE, "row {y}");
				assert_eq!(pixel(&program, 6, y), RED, "row {y}");
			}
			assert_eq!(pixel(&program, 1, 4), RED, "an offline mask hides the item");
		})
		.await;
	}

	/// A blend mode mixes an item with what is under it by the CSS formula,
	/// weighted by the item's opacity.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn blend_modes_mix_with_what_is_under() {
		let cases = [
			(Blend::Multiply, 255, [0, 0, 0]),
			(Blend::Screen, 255, [255, 0, 255]),
			(Blend::Difference, 255, [255, 0, 255]),
			(Blend::Luminosity, 255, [94, 0, 0]),
			(Blend::Multiply, 128, [127, 0, 0]),
		];
		local(async {
			for (blend, alpha, expected) in cases {
				let mut collection = collection();
				collection.sources.insert(
					"tint".into(),
					Source::Color {
						color: Rgba([0, 0, 255, alpha]),
					},
				);
				let fill = Rect::fill(&collection.canvas);
				collection.scenes[0].items = vec![
					Item::new("red", fill),
					Item {
						blend,
						..Item::new("tint", fill)
					},
				];
				let program = drawn(collection, tempfile::tempdir().unwrap()).await;
				let actual = pixel(&program, 3, 3);
				assert!(near(actual, rgb(expected)), "{blend:?} at {alpha}: {actual:?}");
			}
		})
		.await;
	}
}
