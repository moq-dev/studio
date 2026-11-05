//! Draw scenes onto the canvas on the GPU: a preview, a program, and the
//! transition between them.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use kio::clock::Instant;
use moq_video::render::{Renderer, wgpu};

use crate::Error;
use crate::collection::{
	Canvas, Collection, Crop, Direction, Filter, Item, Layer as Place, Rect, Settled, Source, Transition,
};
use crate::effect::{self, Effects, Frame, Lut, Picture, Prepared};
use crate::overlay::{Overlay, Overlays};
use crate::source::{self, Sources};

/// The format of every canvas texture: gamma-encoded RGBA, what
/// `moq_video::render` produces and what its exporter hands an encoder.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Draws a [`Collection`]'s scenes onto its canvas: one texture for the scene
/// in preview, one for the scene on program, and the [`Transition`] from one
/// to the other.
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
///
/// A transition and each item's keyframes run on the clock
/// [`render`](Self::render) is given, so every output drawn from the program,
/// such as the encoder and a recording, sees the same frame of them. An item's
/// keyframes start when its scene goes on program, and separately when it goes
/// into preview, for the preview.
pub struct Compositor {
	device: wgpu::Device,
	queue: wgpu::Queue,
	canvas: Canvas,
	effects: Effects,

	preview: Target,
	program: Target,
	/// The scene coming in, drawn whole so it fades, slides, or is uncovered
	/// as one picture rather than item by item.
	incoming: Target,
	/// The scene going out, drawn whole to slide it off the canvas.
	outgoing: Target,

	layers: HashMap<String, Layer>,
	/// Lays out and paints overlays, started with the first one.
	overlays: Option<Overlays>,
	preview_scene: Option<String>,
	program_scene: Option<String>,
	/// When the preview scene went into preview, or `None` until the next
	/// render, which starts it.
	preview_start: Option<Instant>,
	/// When the program scene went on program, as `preview_start` is.
	program_start: Option<Instant>,
	/// How long the preview scene had been in preview as of the last render:
	/// the moment of its keyframes the preview texture shows.
	preview_shown: Duration,
	switch: Option<Switch>,
}

/// A transition under way from `from` to the program scene.
struct Switch {
	from: Option<String>,
	/// When `from` went on program.
	from_start: Instant,
	start: Instant,
	transition: Transition,
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
	/// An overlay's document, which paints `picture`.
	overlay: Option<Overlay>,
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
	/// Fails if the canvas is invalid or the device cannot hold a texture that
	/// large.
	pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, canvas: Canvas) -> Result<Self, Error> {
		fits(device, canvas)?;

		let effects = Effects::new(device, queue, canvas);
		let target = |label| Target::new(device, &effects, canvas, label);
		Ok(Self {
			preview: target("moq-scene preview"),
			program: target("moq-scene program"),
			incoming: target("moq-scene incoming"),
			outgoing: target("moq-scene outgoing"),
			device: device.clone(),
			queue: queue.clone(),
			canvas,
			effects,
			layers: HashMap::new(),
			overlays: None,
			preview_scene: None,
			program_scene: None,
			preview_start: None,
			program_start: None,
			preview_shown: Duration::ZERO,
			switch: None,
		})
	}

	/// The canvas this compositor draws.
	pub fn canvas(&self) -> Canvas {
		self.canvas
	}

	/// Draw `canvas` from the next [`render`](Self::render) on. A new size
	/// replaces the [`preview`](Self::preview) and [`program`](Self::program)
	/// textures, and every other texture sized from the canvas, with ones of
	/// that size, blank until drawn; a transition under way carries on.
	///
	/// Fails, changing nothing, if the canvas is invalid or the device cannot
	/// hold a texture that large.
	pub fn set_canvas(&mut self, canvas: Canvas) -> Result<(), Error> {
		fits(&self.device, canvas)?;
		self.effects.set_canvas(canvas);
		if canvas.size() != self.canvas.size() {
			let target = |label| Target::new(&self.device, &self.effects, canvas, label);
			self.preview = target("moq-scene preview");
			self.program = target("moq-scene program");
			self.incoming = target("moq-scene incoming");
			self.outgoing = target("moq-scene outgoing");
		}
		self.canvas = canvas;
		Ok(())
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

	/// The scene on program, or the one a transition brings in.
	pub fn program_scene(&self) -> Option<&str> {
		self.program_scene.as_deref()
	}

	/// Why `source`'s newest picture could not be drawn, until one is.
	pub fn error(&self, source: &str) -> Option<&str> {
		self.layers.get(source)?.error.as_deref()
	}

	/// Whether a transition is under way.
	pub fn transitioning(&self) -> bool {
		self.switch.is_some()
	}

	/// The scenes program draws: the one on program, and the one a transition
	/// under way leaves.
	pub fn on_air(&self) -> impl Iterator<Item = &str> {
		let leaving = self.switch.as_ref().and_then(|switch| switch.from.as_deref());
		self.program_scene.as_deref().into_iter().chain(leaving)
	}

	/// Put the scene called `scene` in preview, or nothing, playing its
	/// keyframes from the start, even if it was already there.
	pub fn set_preview(&mut self, scene: Option<String>) {
		self.preview_scene = scene;
		self.preview_start = None;
	}

	/// How long the preview scene had been in preview as of the last
	/// [`render`](Self::render): the moment of its items' keyframes the
	/// [`preview`](Self::preview) shows, for [`Item::posed`].
	pub fn preview_since(&self) -> Duration {
		self.preview_shown
	}

	/// Follow each renamed scene to its new name, wherever it is: in preview,
	/// on program, or leaving in a transition, and drop each removed one, so a
	/// scene that later takes its name does not go on program without a cut.
	/// `renames` maps each old name to its new one, or `None`, as
	/// [`Collection::renames`] reads them, and each scene is looked up once, so
	/// a name one scene gave up and another took is not followed twice.
	pub fn rename(&mut self, renames: &BTreeMap<String, Option<String>>) {
		for scene in self.scenes() {
			if let Some(to) = scene.as_ref().and_then(|name| renames.get(name)) {
				*scene = to.clone();
			}
		}
	}

	/// Drop each scene `collection` does not have from preview, program, and a
	/// transition under way, such as one a reloaded file left out, so a scene
	/// that later takes its name does not go on program without a cut.
	pub fn retain(&mut self, collection: &Collection) {
		for scene in self.scenes() {
			if scene.as_deref().is_some_and(|name| collection.scene(name).is_none()) {
				*scene = None;
			}
		}
	}

	/// The scenes followed by name: in preview, on program, and leaving.
	fn scenes(&mut self) -> impl Iterator<Item = &mut Option<String>> {
		let leaving = self.switch.as_mut().map(|switch| &mut switch.from);
		[&mut self.preview_scene, &mut self.program_scene]
			.into_iter()
			.chain(leaving)
	}

	/// Put the preview on program at once, ending any transition.
	pub fn cut(&mut self) {
		self.switch = None;
		self.program_scene = self.preview_scene.clone();
		self.program_start = None;
	}

	/// Take the preview to program by `transition`, starting at `now`.
	///
	/// A transition already under way lands on its scene first, so this one
	/// starts from what that one was heading to.
	pub fn transition(&mut self, transition: Transition, now: Instant) {
		let from = std::mem::replace(&mut self.program_scene, self.preview_scene.clone());
		let from_start = self.program_start.replace(now).unwrap_or(now);
		self.switch = (!transition.duration().is_zero()).then_some(Switch {
			from,
			from_start,
			start: now,
			transition,
		});
	}

	/// Draw preview and program as of `now`, from the newest picture each of
	/// `sources` has.
	///
	/// A source whose picture fails to draw keeps its last one and reports why
	/// through [`error`](Self::error); this fails only when the canvas itself
	/// cannot be drawn.
	pub fn render(&mut self, collection: &Collection, sources: &Sources, now: Instant) -> Result<(), Error> {
		self.update(collection, sources, now);
		self.paint(collection, now);

		let since = |start: &mut Option<Instant>| now.saturating_duration_since(*start.get_or_insert(now));
		let preview_since = since(&mut self.preview_start);
		self.preview_shown = preview_since;
		let program_since = since(&mut self.program_start);

		let linear = self.switch.as_ref().map(|switch| {
			let elapsed = now.saturating_duration_since(switch.start);
			(elapsed.as_secs_f32() / switch.transition.duration().as_secs_f32()).min(1.0)
		});
		if linear.is_some_and(|linear| linear >= 1.0) {
			self.switch = None;
		}

		let mut frame = self.effects.frame();
		let mut draw = Draw {
			collection,
			effects: &mut self.effects,
			library: Library(&self.layers),
			frame: &mut frame,
			prepared: HashMap::new(),
		};

		// The top layer goes over both outputs, and over a transition rather
		// than through it, so it stays on across the switch.
		draw.target(&self.preview);
		draw.scene(&self.preview, On(self.preview_scene.as_deref(), preview_since));
		draw.layer(&self.preview, Place::Top, Duration::ZERO);

		let to = On(self.program_scene.as_deref(), program_since);
		match (&self.switch, linear) {
			(Some(switch), Some(linear)) => {
				let from = On(switch.from.as_deref(), now.saturating_duration_since(switch.from_start));
				let progress = switch.transition.progress(linear);
				let canvas = Rect::fill(&self.canvas);
				match switch.transition {
					Transition::Fade { .. } => {
						draw.whole(&self.incoming, to);
						draw.target(&self.program);
						draw.scene(&self.program, from);
						draw.place(&self.program, &self.incoming, canvas, canvas, progress);
					}
					// Uncovered where a slide would have moved it in.
					Transition::Wipe { direction, .. } => {
						draw.whole(&self.incoming, to);
						draw.target(&self.program);
						draw.scene(&self.program, from);
						let (x, y) = offset(direction, progress, &canvas);
						let (dx, dy) = offset(direction, 1.0, &canvas);
						let uncovered = shifted(canvas, x - dx, y - dy);
						draw.place(&self.program, &self.incoming, canvas, uncovered, 1.0);
					}
					Transition::Slide { direction, .. } => {
						draw.whole(&self.incoming, to);
						draw.whole(&self.outgoing, from);
						draw.target(&self.program);
						let (x, y) = offset(direction, progress, &canvas);
						let (dx, dy) = offset(direction, 1.0, &canvas);
						draw.place(&self.program, &self.outgoing, shifted(canvas, x, y), canvas, 1.0);
						let entering = shifted(canvas, x - dx, y - dy);
						draw.place(&self.program, &self.incoming, entering, canvas, 1.0);
					}
					Transition::Move { .. } => {
						draw.target(&self.program);
						draw.moving(&self.program, from, to, progress);
					}
				}
			}
			_ => {
				draw.target(&self.program);
				draw.scene(&self.program, to);
			}
		}
		draw.layer(&self.program, Place::Top, Duration::ZERO);

		self.effects.submit(frame);
		Ok(())
	}

	/// Pull the newest picture from every source onto the GPU, and forget the
	/// sources the collection no longer has or has since edited. An overlay
	/// that arrives now starts its CSS timeline at `now`.
	fn update(&mut self, collection: &Collection, sources: &Sources, now: Instant) {
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
				overlay: None,
				error: None,
			});
			// Every picture lands premultiplied, so its texels mix on the
			// sampler's fast path: a page arrives so, moq-video renders a
			// frame opaque, and a still is premultiplied here. Only an
			// overlay's paint is straight, for the shader to premultiply.
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
				source::Picture::Rgba { size, mut pixels } => {
					premultiply(&mut pixels);
					upload(&self.device, &self.queue, size, &pixels)
				}
				source::Picture::Texture(texture) => Ok(texture),
				source::Picture::Lut(cube) => {
					layer.lut = Some(self.effects.lut(&cube));
					continue;
				}
				source::Picture::Overlay => {
					let Source::Overlay { html, width, height } = source else {
						unreachable!("only an overlay source shows an overlay")
					};
					let overlays = match &mut self.overlays {
						Some(overlays) => Ok(overlays),
						None => Overlays::new(&self.device, &self.queue).map(|overlays| self.overlays.insert(overlays)),
					};
					match overlays {
						Ok(overlays) => {
							let overlay = overlays.open(html, *width, *height, now);
							layer.picture = Some(self.effects.picture(overlay.texture(), false));
							layer.overlay = Some(overlay);
						}
						Err(err) => layer.error = Some(err.to_string()),
					}
					continue;
				}
			};

			match texture {
				Ok(texture) => {
					layer.error = None;
					layer.picture = Some(self.effects.picture(&texture, true));
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

	/// Paint each overlay preview or program draws as of `now`, where it moved
	/// since its last paint. One drawn nowhere waits: its CSS runs on the
	/// program clock, so it lands where it should whenever it is drawn again.
	fn paint(&mut self, collection: &Collection, now: Instant) {
		let Some(overlays) = &self.overlays else { return };
		let shown = self.shown(collection);
		let painting: Vec<_> = self
			.layers
			.iter()
			.filter(|(name, _)| shown.contains(name.as_str()))
			.filter_map(|(_, layer)| layer.overlay.as_ref().map(Overlay::id))
			.collect();
		if painting.is_empty() {
			return;
		}

		let failed = overlays.paint(now, painting.clone());
		for (name, layer) in &mut self.layers {
			let Some(overlay) = &layer.overlay else { continue };
			let error = match &failed {
				// The thread is gone, and with it every overlay it held, drawn or
				// not, until each is edited.
				Err(err) => {
					layer.overlay = None;
					layer.picture = None;
					Some(err.to_string())
				}
				Ok(_) if !painting.contains(&overlay.id()) => continue,
				Ok(failed) => failed.get(&overlay.id()).cloned(),
			};
			if let Some(err) = &error
				&& layer.error.is_none()
			{
				tracing::warn!(source = %name, %err, "cannot paint overlay");
			}
			layer.error = error;
		}
		if failed.is_err() {
			self.overlays = None;
		}
	}

	/// The sources whose pictures preview and program draw: each visible
	/// item's, and each mask's, through clones.
	fn shown<'a>(&self, collection: &'a Collection) -> HashSet<&'a str> {
		let leaving = self.switch.as_ref().and_then(|switch| switch.from.as_deref());
		let scenes = [self.preview_scene.as_deref(), self.program_scene.as_deref(), leaving]
			.into_iter()
			.flatten()
			.map(|name| Place::Scene(name.to_string()));
		let mut shown = HashSet::new();
		for place in scenes.chain([Place::Top]) {
			for item in collection
				.items(&place)
				.into_iter()
				.flatten()
				.filter(|item| item.visible)
			{
				shown.insert(collection.capture(&item.source));
				for filter in collection.filters(item) {
					if let Filter::Mask { source, .. } = filter {
						shown.insert(collection.capture(source));
					}
				}
			}
		}
		shown
	}
}

/// How far moving `amount` of `canvas`'s width or height in `direction` goes,
/// in whole pixels so a moving scene stays sharp and two meet without a seam.
fn offset(direction: Direction, amount: f32, canvas: &Rect) -> (f32, f32) {
	let (x, y) = match direction {
		Direction::Left => (-canvas.width, 0.0),
		Direction::Right => (canvas.width, 0.0),
		Direction::Up => (0.0, -canvas.height),
		Direction::Down => (0.0, canvas.height),
	};
	((x * amount).round(), (y * amount).round())
}

/// `rect` moved `x` right and `y` down.
fn shifted(rect: Rect, x: f32, y: f32) -> Rect {
	Rect {
		x: rect.x + x,
		y: rect.y + y,
		..rect
	}
}

/// Where an item is drawn and how much of it shows: what a move transition
/// changes as it goes.
#[derive(Clone, Copy, PartialEq)]
struct Pose {
	rect: Rect,
	crop: Crop,
	opacity: f32,
}

impl Pose {
	/// `item` where `collection` places it.
	fn of(collection: &Collection, item: &Item) -> Self {
		Self {
			rect: item.rect,
			crop: collection.crop(item),
			opacity: 1.0,
		}
	}

	/// `progress` of the way from `self` to `other`.
	fn toward(&self, other: &Self, progress: f32) -> Self {
		let mix = |from: f32, to: f32| crate::collection::mix(from, to, progress);
		Self {
			rect: Rect {
				x: mix(self.rect.x, other.rect.x),
				y: mix(self.rect.y, other.rect.y),
				width: mix(self.rect.width, other.rect.width),
				height: mix(self.rect.height, other.rect.height),
			},
			crop: self.crop.toward(&other.crop, progress),
			opacity: mix(self.opacity, other.opacity),
		}
	}
}

/// `old`'s filters `progress` of the way to `new`'s, or `None` if their blend
/// modes differ or their filters do not pair up by kind, in order, as
/// [`Filter::toward`] mixes them.
fn mixed(old: &Item, new: &Item, progress: f32) -> Option<Vec<Filter>> {
	if old.blend != new.blend || old.filters.len() != new.filters.len() {
		return None;
	}
	let pairs = old.filters.iter().zip(&new.filters);
	pairs.map(|(old, new)| old.toward(new, progress)).collect()
}

/// The order to draw a move's visible items in, back to front, as `lead`
/// stacks them: each slot is an item of `lead`, of `other`, or a pair of
/// both, as `lead_pairs` and `other_pairs` pair them. An unpaired item of
/// `other` goes just over the paired item it was over in `other`, or under
/// every slot if it was under every paired item.
fn stack(
	lead: &[Cow<'_, Item>],
	lead_pairs: &[Option<usize>],
	other: &[Cow<'_, Item>],
	other_pairs: &[Option<usize>],
) -> Vec<(Option<usize>, Option<usize>)> {
	let mut over: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
	let mut under = None;
	for index in (0..other.len()).filter(|&index| other[index].visible) {
		match other_pairs[index] {
			Some(_) => under = Some(index),
			None => over.entry(under).or_default().push(index),
		}
	}
	let mut unpaired = |under: Option<usize>| {
		let items = over.remove(&under).unwrap_or_default();
		items.into_iter().map(|index| (None, Some(index)))
	};

	let mut slots: Vec<_> = unpaired(None).collect();
	for index in (0..lead.len()).filter(|&index| lead[index].visible) {
		let pair = lead_pairs[index];
		slots.push((Some(index), pair));
		if pair.is_some() {
			slots.extend(unpaired(pair));
		}
	}
	slots
}

/// Refuse an invalid canvas, or one larger than `device` can hold as a
/// texture.
fn fits(device: &wgpu::Device, canvas: Canvas) -> Result<(), Error> {
	canvas.validate()?;
	let limit = device.limits().max_texture_dimension_2d;
	if canvas.width > limit || canvas.height > limit {
		return Err(Error::Invalid(format!(
			"canvas {}x{} exceeds this GPU's {limit}px texture limit",
			canvas.width, canvas.height
		)));
	}
	Ok(())
}

/// A scene to draw, or none, and how long it has been on: where its items'
/// keyframes are.
#[derive(Clone, Copy)]
struct On<'a>(Option<&'a str>, Duration);

/// One render's draws onto its targets.
struct Draw<'a> {
	collection: &'a Collection,
	effects: &'a mut Effects,
	library: Library<'a>,
	frame: &'a mut Frame,
	/// Each item, filtered once though it is drawn onto several targets, by
	/// where it is and its [`Item::settled`] time, as two outputs showing its
	/// scene can be at different points of its keyframes.
	prepared: HashMap<(Place, usize, Settled), Option<Prepared>>,
}

impl Draw<'_> {
	/// Start drawing `target`.
	fn target(&mut self, target: &Target) {
		self.frame.target(&target.picture);
	}

	/// Draw `on`'s items onto `target`. A missing scene draws nothing.
	fn scene(&mut self, target: &Target, on: On) {
		if let Some(scene) = on.0.filter(|name| self.collection.scene(name).is_some()) {
			self.layer(target, Place::Scene(scene.to_string()), on.1);
		}
	}

	/// Start drawing `target`, and draw `on` onto it whole.
	fn whole(&mut self, target: &Target, on: On) {
		self.target(target);
		self.scene(target, on);
	}

	/// Draw `picture`, a canvas-sized target drawn this render, onto `target`
	/// stretched over `at`, showing only within `within`, at `opacity`.
	fn place(&mut self, target: &Target, picture: &Target, at: Rect, within: Rect, opacity: f32) {
		if let Some(placed) = self.effects.place(&picture.picture, at, within, opacity) {
			self.effects.draw(self.frame, &target.texture, &placed);
		}
	}

	/// Draw `place`'s items onto `target`, back to front, each posed `since`
	/// `place` went on, through its crop, filters, and blend mode. A hidden
	/// item, a source with no picture yet, or a filter whose source has nothing
	/// yet draws nothing.
	fn layer(&mut self, target: &Target, place: Place, since: Duration) {
		let items = self.collection.items(&place).map_or(&[][..], Vec::as_slice);
		for (index, item) in items.iter().enumerate().filter(|(_, item)| item.visible) {
			self.item(target, (place.clone(), index, item.settled(since)), &item.posed(since));
		}
	}

	/// Draw `item`, posed, where it is placed onto `target`, shared with every
	/// other draw of it under `key`.
	fn item(&mut self, target: &Target, key: (Place, usize, Settled), item: &Item) {
		let prepared = match self.prepared.get(&key) {
			Some(prepared) => prepared.clone(),
			None => {
				let prepared = self.prepare(item, Pose::of(self.collection, item));
				self.prepared.insert(key, prepared.clone());
				prepared
			}
		};
		if let Some(prepared) = prepared {
			self.effects.draw(self.frame, &target.texture, &prepared);
		}
	}

	/// Draw `item` onto `target` in `pose` rather than where it is placed.
	fn posed(&mut self, target: &Target, item: &Item, pose: Pose) {
		if pose.opacity <= 0.0 {
			return;
		}
		if let Some(prepared) = self.prepare(item, pose) {
			self.effects.draw(self.frame, &target.texture, &prepared);
		}
	}

	/// Draw `progress` of a move from scene `from` to scene `to` onto
	/// `target`, each item posed by its keyframes as its scene is.
	///
	/// Each visible item of `to` is paired with the first visible item of
	/// `from` showing the same source that no earlier one took, and glides
	/// from its pose to its own, its filters' settings mixing from its look in
	/// `from` to its look in `to`. The rest of `from`
	/// fades out and the rest of `to` fades in, where each was placed. Items
	/// stack as `from` stacks them until halfway, then as `to` does, each
	/// unpaired item just over the paired item it was over, so the move
	/// starts from `from` and lands on `to` as each is drawn. A paired item
	/// whose blend mode or kinds of filter differ changes look halfway too.
	fn moving(&mut self, target: &Target, from: On, to: On, progress: f32) {
		let collection = self.collection;
		let items = |scene: Option<&str>| {
			let scene = scene.and_then(|name| collection.scene(name));
			scene.map_or(&[][..], |scene| scene.items.as_slice())
		};
		let posed = |on: On| -> Vec<Cow<'_, Item>> { items(on.0).iter().map(|item| item.posed(on.1)).collect() };
		let resting = items(to.0);
		let (leaving, entering) = (posed(from), posed(to));

		// Which item of the other scene each visible item pairs with.
		let mut left = vec![None; leaving.len()];
		let mut entered = vec![None; entering.len()];
		for (index, item) in entering.iter().enumerate().filter(|(_, item)| item.visible) {
			let pair = (0..leaving.len())
				.find(|&at| left[at].is_none() && leaving[at].visible && leaving[at].source == item.source);
			if let Some(at) = pair {
				left[at] = Some(index);
				entered[index] = Some(at);
			}
		}

		// Each slot is an index into `leaving`, `entering`, or both.
		let slots: Vec<(Option<usize>, Option<usize>)> = if progress < 0.5 {
			stack(&leaving, &left, &entering, &entered)
		} else {
			let slots = stack(&entering, &entered, &leaving, &left);
			slots.into_iter().map(|(to, from)| (from, to)).collect()
		};

		let place = Place::Scene(to.0.unwrap_or_default().to_string());
		for slot in slots {
			match slot {
				(Some(at), None) => {
					let pose = Pose::of(collection, &leaving[at]);
					let opacity = 1.0 - progress;
					self.posed(target, &leaving[at], Pose { opacity, ..pose });
				}
				(None, Some(index)) => {
					let pose = Pose::of(collection, &entering[index]);
					self.posed(
						target,
						&entering[index],
						Pose {
							opacity: progress,
							..pose
						},
					);
				}
				(Some(at), Some(index)) => {
					let (old, new) = (&*leaving[at], &*entering[index]);
					let own = Pose::of(collection, new);
					let pose = Pose::of(collection, old).toward(&own, progress);
					if old.filters == new.filters && old.blend == new.blend {
						if pose == own {
							// Shared with the preview when it draws the same.
							let key = (place.clone(), index, resting[index].settled(to.1));
							self.item(target, key, new);
						} else {
							self.posed(target, new, pose);
						}
					} else if let Some(filters) = mixed(old, new, progress) {
						let item = Item { filters, ..new.clone() };
						self.posed(target, &item, pose);
					} else {
						// A look that cannot be mixed changes as the stacking does.
						self.posed(target, if progress < 0.5 { old } else { new }, pose);
					}
				}
				(None, None) => unreachable!("a slot holds an item"),
			}
		}
	}

	fn prepare(&mut self, item: &Item, pose: Pose) -> Option<Prepared> {
		let collection = self.collection;
		let picture = self.library.0.get(collection.capture(&item.source))?.picture.as_ref()?;
		let fade = Filter::Opacity { amount: pose.opacity };
		let mut filters: Vec<&Filter> = collection.filters(item).collect();
		if pose.opacity < 1.0 {
			filters.push(&fade);
		}
		let item = effect::Item {
			picture,
			rect: pose.rect,
			crop: pose.crop,
			filters,
			blend: item.blend,
		};
		self.effects.prepare(self.frame, item, &self.library)
	}
}

/// Premultiply tightly packed RGBA in place.
fn premultiply(pixels: &mut [u8]) {
	for pixel in pixels.chunks_exact_mut(4) {
		let alpha = u16::from(pixel[3]);
		for channel in &mut pixel[..3] {
			*channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
		}
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
		// A canvas is opaque, which is premultiplied too.
		let picture = effects.picture(&texture, true);
		Self { texture, picture }
	}
}

#[cfg(test)]
mod tests {
	use std::time::Duration;

	use super::*;
	use crate::collection::{Blend, MaskMode, Rgba, Scene};
	use crate::source::test::{context, local, start};

	/// A fade over a second.
	const FADE: Transition = Transition::Fade {
		duration: Duration::from_secs(1),
	};

	const RED: [u8; 4] = [255, 0, 0, 255];
	const BLUE: [u8; 4] = [0, 0, 255, 255];
	const GREEN: [u8; 4] = [0, 255, 0, 255];
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
			studio.compositor.transition(FADE, start);
			assert_eq!(studio.compositor.program_scene(), Some("blue"));

			studio.render(start + Duration::from_millis(500));
			assert!(studio.compositor.transitioning());
			let program = studio.program();
			let actual = pixel(&program, 1, 1);
			assert!(near(actual, [128, 0, 127, 255]), "{actual:?}");
			assert_eq!(pixel(&program, 6, 1), BLUE, "blue under blue stays blue");
			assert_eq!(pixel(&studio.preview(), 1, 1), BLUE);

			studio.render(start + Duration::from_secs(1));
			assert!(!studio.compositor.transitioning());
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
			studio.compositor.transition(FADE, start);
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
			studio.compositor.transition(FADE, start);
			studio.render(start + Duration::from_millis(500));
			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			assert!(!studio.compositor.transitioning());
			studio.render(start + Duration::from_millis(600));
			assert_eq!(pixel(&studio.program(), 1, 1), RED);
		})
		.await;
	}

	/// Halfway through a slide to the left, eased to half the canvas, the
	/// outgoing scene's right half shows on the left and the incoming scene's
	/// left half on the right; a wipe leaves the outgoing scene in place and
	/// uncovers the incoming one where the slide brought it in.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_slide_pushes_and_a_wipe_uncovers() {
		local(async {
			let mut collection = collection();
			collection.sources.insert(
				"green".into(),
				Source::Color {
					color: Rgba([0, 255, 0, 255]),
				},
			);
			collection.scenes[1].items = vec![Item::new(
				"green",
				Rect {
					x: 0.0,
					y: 0.0,
					width: 4.0,
					height: 8.0,
				},
			)];
			let mut studio = Studio::new(collection).await;
			let duration = Duration::from_secs(1);
			for (transition, expected) in [
				(
					Transition::Slide {
						duration,
						direction: Direction::Left,
					},
					[BLUE, RED, GREEN, GREEN],
				),
				(
					Transition::Wipe {
						duration,
						direction: Direction::Left,
					},
					[RED, RED, BLACK, BLACK],
				),
				(
					Transition::Wipe {
						duration,
						direction: Direction::Down,
					},
					[GREEN, RED, BLACK, RED],
				),
			] {
				let start = Instant::now();
				studio.compositor.set_preview(Some("red".into()));
				studio.compositor.cut();
				studio.compositor.set_preview(Some("blue".into()));
				studio.compositor.transition(transition, start);
				studio.render(start + Duration::from_millis(500));
				let program = studio.program();
				let actual = [(1, 1), (1, 6), (6, 1), (6, 6)].map(|(x, y)| pixel(&program, x, y));
				assert_eq!(actual, expected, "{transition:?}");
				assert_eq!(pixel(&studio.preview(), 1, 1), GREEN, "preview stays put");
			}
		})
		.await;
	}

	/// A move glides an item both scenes place from one place to the other,
	/// fades out what only the outgoing scene places, still over what it was
	/// over, and fades in what only the incoming one does, landing on the
	/// incoming scene as drawn.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_move_glides_what_both_scenes_place() {
		local(async {
			let mut collection = collection();
			collection.sources.insert(
				"green".into(),
				Source::Color {
					color: Rgba([0, 255, 0, 255]),
				},
			);
			let tile = Rect {
				x: 6.0,
				y: 0.0,
				width: 2.0,
				height: 2.0,
			};
			collection
				.sources
				.insert("white".into(), Source::Color { color: Rgba([255; 4]) });
			let badge = Rect {
				x: 4.0,
				y: 2.0,
				width: 2.0,
				height: 2.0,
			};
			collection.scenes[0].items.push(Item::new("white", badge));
			collection.scenes[1].items = vec![Item::new("blue", CORNER), Item::new("green", tile)];
			let mut studio = Studio::new(collection).await;
			let start = Instant::now();
			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			studio.compositor.set_preview(Some("blue".into()));
			let duration = Duration::from_secs(1);
			studio.compositor.transition(Transition::Move { duration }, start);

			// Eased halfway, blue is at (2, 2) and both fades are at half.
			studio.render(start + Duration::from_millis(500));
			let program = studio.program();
			assert_eq!(pixel(&program, 3, 3), BLUE, "blue in between");
			assert_eq!(pixel(&program, 5, 4), BLUE);
			let actual = pixel(&program, 5, 3);
			assert!(near(actual, [128, 128, 255, 255]), "white half over blue: {actual:?}");
			let actual = pixel(&program, 1, 6);
			assert!(near(actual, [128, 0, 0, 255]), "red half gone: {actual:?}");
			let actual = pixel(&program, 7, 0);
			assert!(near(actual, [64, 128, 0, 255]), "green half over half red: {actual:?}");

			studio.render(start + duration);
			assert!(!studio.compositor.transitioning());
			let program = studio.program();
			assert_eq!(pixel(&program, 1, 6), BLUE);
			assert_eq!(pixel(&program, 6, 6), BLACK);
			assert_eq!(pixel(&program, 7, 0), GREEN);
		})
		.await;
	}

	/// Keyframes sliding an item in from off the canvas's left over a second.
	fn slide_in() -> Vec<crate::Keyframe> {
		vec![
			crate::Keyframe {
				x: -8.0,
				..crate::Keyframe::new(Duration::ZERO)
			},
			crate::Keyframe {
				ease: crate::Ease::Linear,
				..crate::Keyframe::new(Duration::from_secs(1))
			},
		]
	}

	/// An item's keyframes play from when its scene went into preview, for the
	/// preview, and from when it went on program, for the program, so one scene
	/// can be at two points of them at once.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn keyframes_play_from_when_the_scene_goes_on() {
		local(async {
			let mut collection = collection();
			collection.scenes[1].items[0].keyframes = slide_in();
			let mut studio = Studio::new(collection).await;
			let start = Instant::now();
			let at = |millis| start + Duration::from_millis(millis);
			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			studio.compositor.set_preview(Some("blue".into()));

			studio.render(at(0));
			assert_eq!(pixel(&studio.preview(), 1, 6), BLACK, "off the canvas");
			studio.render(at(500));
			assert_eq!(
				studio.compositor.preview_since(),
				Duration::from_millis(500),
				"the frame shown"
			);
			let preview = studio.preview();
			assert_eq!(pixel(&preview, 1, 6), BLUE, "halfway in");
			assert_eq!(pixel(&preview, 6, 6), BLACK);

			studio.compositor.cut();
			studio.render(at(500));
			assert_eq!(pixel(&studio.program(), 1, 6), BLACK, "on program from the start");
			assert_eq!(pixel(&studio.preview(), 1, 6), BLUE, "the preview carries on");

			studio.render(at(1000));
			let program = studio.program();
			assert_eq!(pixel(&program, 1, 6), BLUE);
			assert_eq!(pixel(&program, 6, 6), BLACK);
			assert_eq!(pixel(&studio.preview(), 6, 6), BLUE, "landed where it rests");
			studio.render(at(9000));
			assert_eq!(pixel(&studio.program(), 6, 6), BLUE, "holding the last keyframe");

			// Picking the scene again plays it again.
			studio.compositor.set_preview(Some("blue".into()));
			studio.render(at(9000));
			assert_eq!(pixel(&studio.preview(), 1, 6), BLACK);
		})
		.await;
	}

	/// Two outputs at either side of a jump, keyframes at the same moment, each
	/// draw their own side of it rather than share one.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_jump_draws_each_side_on_its_own_output() {
		local(async {
			let mut collection = collection();
			let jump = Duration::from_secs(1);
			collection.scenes[1].items[0].keyframes = vec![
				crate::Keyframe {
					x: -8.0,
					..crate::Keyframe::new(jump)
				},
				crate::Keyframe::new(jump),
			];
			let mut studio = Studio::new(collection).await;
			let start = Instant::now();
			studio.compositor.set_preview(Some("blue".into()));
			studio.compositor.cut();
			studio.render(start);
			// Program has reached the jump; preview starts over before it.
			studio.compositor.set_preview(Some("blue".into()));
			studio.render(start + jump);
			assert_eq!(pixel(&studio.preview(), 1, 6), BLACK, "before the jump");
			assert_eq!(pixel(&studio.program(), 1, 6), BLUE, "after the jump");
		})
		.await;
	}

	/// A transition starts the incoming scene's keyframes, however long it was
	/// in preview, while the outgoing scene's carry on.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_transition_starts_the_incoming_keyframes() {
		local(async {
			let mut collection = collection();
			collection.scenes[0].items[0].keyframes = slide_in();
			collection.scenes[1].items[0].keyframes = slide_in();
			let mut studio = Studio::new(collection).await;
			let start = Instant::now();
			let at = |millis| start + Duration::from_millis(millis);
			studio.compositor.set_preview(Some("blue".into()));
			studio.compositor.cut();
			studio.compositor.set_preview(Some("red".into()));
			studio.render(at(0));

			studio.compositor.transition(FADE, at(9000));
			studio.render(at(9500));
			let program = studio.program();
			// Red has slid halfway in over the left half, blue is in place, and
			// the fade is halfway.
			let actual = pixel(&program, 1, 6);
			assert!(near(actual, [128, 0, 128, 255]), "red half over blue: {actual:?}");
			let actual = pixel(&program, 6, 6);
			assert!(near(actual, [0, 0, 128, 255]), "nothing half over blue: {actual:?}");
		})
		.await;
	}

	/// A move starts from the outgoing scene's stacking and switches to the
	/// incoming scene's halfway, and an item's filter settings mix from one
	/// scene's to the other's as it goes.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_move_restacks_halfway_and_mixes_filters() {
		local(async {
			let mut collection = collection();
			let fill = Rect::fill(&collection.canvas);
			let invert = |amount| Item {
				filters: vec![Filter::Invert { amount }],
				..Item::new("blue", fill)
			};
			collection.scenes[0].items = vec![Item::new("red", CORNER), invert(0.0)];
			collection.scenes[1].items = vec![invert(1.0), Item::new("red", CORNER)];
			let mut studio = Studio::new(collection).await;
			let start = Instant::now();
			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			studio.compositor.set_preview(Some("blue".into()));
			let duration = Duration::from_secs(1);
			studio.compositor.transition(Transition::Move { duration }, start);

			studio.render(start);
			let program = studio.program();
			assert_eq!(pixel(&program, 1, 6), BLUE, "blue over red, as the outgoing scene");
			assert_eq!(pixel(&program, 6, 1), BLUE);

			studio.render(start + Duration::from_millis(500));
			let program = studio.program();
			assert_eq!(pixel(&program, 1, 6), RED, "red over blue from halfway");
			let actual = pixel(&program, 6, 1);
			assert!(near(actual, [128, 128, 128, 255]), "half inverted: {actual:?}");

			studio.render(start + duration);
			assert_eq!(pixel(&studio.program(), 6, 1), [255, 255, 0, 255]);
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

	/// A scaled picture mixes its texels premultiplied, so the color of a
	/// transparent texel bleeds into no edge, of a still, an overlay's
	/// straight paint, or a luminance mask, while the edge still fades.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn scaling_bleeds_no_transparent_color() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			// Red beside a transparent green, which straight mixing tints the
			// edge with.
			image::RgbaImage::from_fn(2, 1, |x, _| image::Rgba(if x == 0 { RED } else { [0, 255, 0, 0] }))
				.save(dir.path().join("logo.png"))
				.unwrap();
			// White, black, then a transparent white, which straight mixing
			// lets show through where the mask is black.
			image::RgbaImage::from_fn(3, 1, |x, _| {
				image::Rgba([[255; 4], BLACK, [255, 255, 255, 0]][x as usize])
			})
			.save(dir.path().join("mask.png"))
			.unwrap();
			let mut collection = collection();
			for name in ["logo", "mask"] {
				collection.sources.insert(
					name.into(),
					Source::Image {
						path: format!("{name}.png").into(),
					},
				);
			}
			// White beside transparent, painted straight.
			collection.sources.insert(
				"overlay".into(),
				Source::Overlay {
					html: "<style>
						body { margin: 0 }
						div { width: 1px; height: 1px; background: white }
					</style><div></div>"
						.into(),
					width: 2,
					height: 1,
				},
			);
			let row = |source: &str, y| {
				Item::new(
					source,
					Rect {
						x: 0.0,
						y,
						width: 8.0,
						height: 2.0,
					},
				)
			};
			collection.scenes[0].items = vec![
				row("logo", 0.0),
				row("overlay", 2.0),
				Item {
					filters: vec![Filter::Mask {
						source: "mask".into(),
						mode: MaskMode::Luminance,
					}],
					..row("blue", 4.0)
				},
			];
			let program = drawn(collection, dir).await;

			for x in 0..8 {
				let logo = pixel(&program, x, 0);
				assert_eq!(logo[1..3], [0, 0], "logo at {x}: {logo:?}");
				// The overlay's edge fades as the logo's does.
				let overlay = pixel(&program, x, 2);
				assert!(
					near(overlay, rgb([logo[0]; 3])),
					"overlay at {x}: {overlay:?}, logo {logo:?}"
				);
			}
			assert_eq!(pixel(&program, 0, 0), RED);
			let edge = pixel(&program, 3, 0)[0];
			assert!(0 < edge && edge < 255, "the logo's edge fades: {edge}");
			assert_eq!(pixel(&program, 7, 0), BLACK);

			assert_eq!(pixel(&program, 0, 4), BLUE);
			for x in 4..8 {
				assert_eq!(pixel(&program, x, 4), BLACK, "mask at {x}");
			}
		})
		.await;
	}

	/// A resize draws the next frame on textures of the new size, with items
	/// where their pixels say, a blend mode and a blur reaching the canvas's
	/// new width, and a canvas the GPU cannot hold changes nothing.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_resize_draws_the_new_canvas() {
		local(async {
			let mut collection = collection();
			collection.sources.insert(
				"tint".into(),
				Source::Color {
					color: Rgba([0, 0, 255, 255]),
				},
			);
			let mut studio = Studio::new(collection).await;
			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			studio.render(Instant::now());
			let old = studio.compositor.program().clone();

			let wide = Canvas {
				width: 16,
				..studio.collection.canvas
			};
			studio.compositor.set_canvas(wide).unwrap();
			assert_ne!(studio.compositor.program(), &old, "a new texture");
			studio.collection.canvas = wide;
			let fill = Rect::fill(&wide);
			studio.collection.scenes[0].items = vec![
				Item::new("red", Rect { width: 8.0, ..fill }),
				Item {
					blend: Blend::Screen,
					filters: vec![Filter::Blur { radius: 1.0 }],
					..Item::new("tint", fill)
				},
			];
			studio.render(Instant::now());
			let program = studio.program();
			assert_eq!(program.len(), 16 * 8 * 4);
			let pixel = |x: usize, y: usize| -> [u8; 4] {
				let at = (y * 16 + x) * 4;
				program[at..at + 4].try_into().unwrap()
			};
			assert!(
				near(pixel(4, 4), [255, 0, 255, 255]),
				"screened over red: {:?}",
				pixel(4, 4)
			);
			assert!(
				near(pixel(12, 4), BLUE),
				"screened past the old width: {:?}",
				pixel(12, 4)
			);

			let limit = studio.compositor.device.limits().max_texture_dimension_2d;
			let huge = Canvas {
				width: limit + 2,
				..wide
			};
			assert!(matches!(studio.compositor.set_canvas(huge), Err(Error::Invalid(_))));
			let empty = Canvas { width: 0, ..wide };
			assert!(matches!(studio.compositor.set_canvas(empty), Err(Error::Invalid(_))));
			assert_eq!(studio.compositor.canvas(), wide);
		})
		.await;
	}

	/// A renamed scene stays in preview, on program, and in the fade under way,
	/// even when another scene takes its old name in the same edit, and a scene
	/// the collection lost leaves them.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_renamed_scene_stays_where_it_is() {
		local(async {
			let mut studio = Studio::new(collection()).await;
			let start = Instant::now();
			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			studio.compositor.set_preview(Some("blue".into()));
			studio.compositor.transition(FADE, start);

			// `blue` takes the name `red` gives up, which is followed once.
			let mut draft = studio.collection.draft("Rename");
			draft.rename_scene(0, "crimson").unwrap();
			draft.rename_scene(1, "red").unwrap();
			let edit = draft.finish().unwrap();
			studio.compositor.rename(&studio.collection.renames(&edit));
			studio.collection.apply(&edit).unwrap();
			assert_eq!(studio.compositor.preview_scene(), Some("red"));
			assert_eq!(studio.compositor.program_scene(), Some("red"));
			studio.render(start + Duration::from_millis(500));
			let actual = pixel(&studio.program(), 1, 1);
			assert!(near(actual, [128, 0, 127, 255]), "still fading from red: {actual:?}");

			// A scene the collection no longer has, as a reload can leave, is
			// dropped, so one that later takes its name is not drawn unasked.
			studio.collection.scenes.retain(|scene| scene.name != "red");
			studio.compositor.retain(&studio.collection);
			assert_eq!(studio.compositor.preview_scene(), None);
			assert_eq!(studio.compositor.program_scene(), None);
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

	/// An overlay's CSS animation runs on the program clock from when the
	/// overlay opens: a frame shows it where the program time says, however late
	/// that frame is drawn, even the first one to show it.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn an_overlay_animates_on_the_program_clock() {
		local(async {
			let mut collection = collection();
			collection.sources.insert(
				"overlay".into(),
				Source::Overlay {
					html: "<style>
						body { margin: 0 }
						div { width: 8px; height: 4px; animation: fade 1s linear both }
						@keyframes fade { from { background: red } to { background: blue } }
					</style><div></div>"
						.into(),
					width: 8,
					height: 8,
				},
			);
			collection.scenes[1].items = vec![Item::new("overlay", Rect::fill(&collection.canvas))];
			let mut studio = Studio::new(collection).await;
			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			// Opened here, but drawn nowhere until the cut.
			let start = Instant::now();
			studio.render(start);
			studio.compositor.set_preview(Some("blue".into()));
			studio.compositor.cut();

			for (at, expected) in [(500, [128, 0, 128, 255]), (750, [64, 0, 191, 255]), (2000, BLUE)] {
				studio.render(start + Duration::from_millis(at));
				let program = studio.program();
				let actual = pixel(&program, 3, 1);
				assert!(
					actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 2),
					"{at} ms: {actual:?}"
				);
				assert_eq!(pixel(&program, 3, 6), BLACK, "{at} ms: the rest is transparent");
			}
		})
		.await;
	}

	/// An overlay a move glides is painted on the program clock while both
	/// scenes are on air, and drawn where the move has it.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_move_glides_an_overlay() {
		local(async {
			let mut collection = collection();
			collection.sources.insert(
				"overlay".into(),
				Source::Overlay {
					html: "<body style=\"margin: 0; background: lime\"></body>".into(),
					width: 4,
					height: 4,
				},
			);
			let tile = |x, y| Rect {
				x,
				y,
				width: 2.0,
				height: 2.0,
			};
			collection.scenes[0].items = vec![Item::new("overlay", tile(0.0, 0.0))];
			collection.scenes[1].items = vec![Item::new("overlay", tile(4.0, 4.0))];
			let mut studio = Studio::new(collection).await;
			let start = Instant::now();
			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			studio.compositor.set_preview(Some("blue".into()));
			let duration = Duration::from_secs(1);
			studio.compositor.transition(Transition::Move { duration }, start);

			studio.render(start + Duration::from_millis(500));
			let program = studio.program();
			assert_eq!(pixel(&program, 2, 2), GREEN, "halfway, at (2, 2)");
			assert_eq!(pixel(&program, 0, 0), BLACK);
			assert_eq!(pixel(&program, 5, 5), BLACK);

			studio.render(start + duration);
			assert_eq!(pixel(&studio.program(), 5, 5), GREEN);
		})
		.await;
	}

	/// When the overlay thread stops, every overlay it held fails, including
	/// one off air, rather than showing its last paint when cut to.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn a_stopped_overlay_thread_fails_every_overlay() {
		local(async {
			let mut collection = collection();
			let overlay = |color: &str| Source::Overlay {
				html: format!("<body style=\"margin: 0; background: {color}\"></body>"),
				width: 8,
				height: 8,
			};
			collection.sources.insert("shown".into(), overlay("lime"));
			collection.sources.insert("aside".into(), overlay("yellow"));
			let fill = Rect::fill(&collection.canvas);
			collection.scenes[0].items = vec![Item::new("red", fill), Item::new("aside", fill)];
			collection.scenes[1].items = vec![Item::new("blue", fill), Item::new("shown", fill)];
			let mut studio = Studio::new(collection).await;
			let start = Instant::now();

			// Paint the aside overlay, then leave it off air.
			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			studio.render(start);
			assert_eq!(pixel(&studio.program(), 3, 3), [255, 255, 0, 255]);
			studio.compositor.set_preview(Some("blue".into()));
			studio.compositor.cut();
			studio.render(start);

			studio.compositor.overlays.as_ref().unwrap().panic();
			studio.render(start);
			for name in ["shown", "aside"] {
				let err = studio.compositor.error(name).unwrap_or_else(|| panic!("{name} fails"));
				assert!(err.contains("overlay thread stopped"), "{name}: {err}");
			}

			studio.compositor.set_preview(Some("red".into()));
			studio.compositor.cut();
			studio.render(start);
			assert_eq!(pixel(&studio.program(), 3, 3), RED, "no frozen overlay");
		})
		.await;
	}
}
