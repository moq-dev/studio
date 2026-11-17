//! Whether a capture went dark: its picture all one dark color while its
//! source claims to be live, such as a window that closed or changed how it
//! draws, a locked screen, or a camera behind its shutter.
//!
//! A camera that stops delivering already reads as stalled, but a display or
//! window that stands still is no fault, so a capture is judged by what it
//! shows. A [`Probe`] reads the newest picture at most every [`INTERVAL`],
//! on the GPU: a compute shader reduces it to a small grid of tiles, and only
//! the grid comes back, a frame or so later, to be judged on the CPU.
//!
//! Until a capture has been dark for [`DARK`], the probe holds the last
//! picture it judged not dark, for the compositor to draw in its place, so a
//! moment of black, such as a game switching modes, never reaches viewers.
//! After that the capture shows as it is, so a stale picture never passes for
//! live.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use kio::clock::Instant;

use crate::effect::{Effects, Picture};

/// How often a probe reads a capture's picture, at most, and only when one
/// arrives: a camera or window that stops delivering is not read, though a
/// Linux screen capture repeats its last picture while it stands still, and
/// is read each time.
pub const INTERVAL: Duration = Duration::from_millis(250);

/// How long a capture's picture stays dark before it counts as gone dark,
/// rather than a moment of black the last good picture covers.
pub const DARK: Duration = Duration::from_secs(3);

/// The grid a picture is reduced to, as `dark.wgsl`'s `TILES`: about 30
/// texels a side at 1080p.
const TILES: (u32, u32) = (64, 36);

/// The bytes of the reduced grid, four a tile.
const BYTES: u64 = (TILES.0 * TILES.1 * 4) as u64;

/// The brightest a dark picture's brightest channel gets, out of 255:
/// a camera's noise in the dark stays under it, and any lit pixel passes it.
const BRIGHTEST: u8 = 40;

/// How many tiles of a dark picture may be lit anyway: a mouse pointer the
/// capture draws over the black spans up to four.
const LIT: usize = 4;

/// How far apart a dark picture's tiles' mean colors get, out of 255: a dark
/// page with a line of text on it spreads further.
const SPREAD: u8 = 8;

/// Whether a picture reduced to `tiles`, each RGBA with its mean color and
/// its brightest channel as alpha, is all one dark color.
fn judge(tiles: &[u8]) -> bool {
	let mut low = [u8::MAX; 3];
	let mut high = [0u8; 3];
	let mut lit = 0;
	for tile in tiles.as_chunks::<4>().0 {
		if tile[3] > BRIGHTEST {
			lit += 1;
			if lit > LIT {
				return false;
			}
			continue;
		}
		for channel in 0..3 {
			low[channel] = low[channel].min(tile[channel]);
			high[channel] = high[channel].max(tile[channel]);
		}
	}
	(0..3).all(|channel| high[channel].saturating_sub(low[channel]) <= SPREAD)
}

/// The pipeline every probe reduces its pictures with.
pub(crate) struct Prober {
	pipeline: wgpu::ComputePipeline,
	layout: wgpu::BindGroupLayout,
}

impl Prober {
	pub fn new(device: &wgpu::Device) -> Self {
		let shader = device.create_shader_module(wgpu::include_wgsl!("dark.wgsl"));
		let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("moq-scene dark"),
			entries: &[
				wgpu::BindGroupLayoutEntry {
					binding: 0,
					visibility: wgpu::ShaderStages::COMPUTE,
					ty: wgpu::BindingType::Texture {
						sample_type: wgpu::TextureSampleType::Float { filterable: false },
						view_dimension: wgpu::TextureViewDimension::D2,
						multisampled: false,
					},
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 1,
					visibility: wgpu::ShaderStages::COMPUTE,
					ty: wgpu::BindingType::Buffer {
						ty: wgpu::BufferBindingType::Storage { read_only: false },
						has_dynamic_offset: false,
						min_binding_size: wgpu::BufferSize::new(BYTES),
					},
					count: None,
				},
			],
		});
		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("moq-scene dark"),
			bind_group_layouts: &[Some(&layout)],
			immediate_size: 0,
		});
		let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
			label: Some("moq-scene dark"),
			layout: Some(&pipeline_layout),
			module: &shader,
			entry_point: Some("reduce"),
			compilation_options: Default::default(),
			cache: None,
		});
		Self { pipeline, layout }
	}
}

/// A read under way: when its picture was taken, and the buffer's mapping
/// once the GPU is done.
struct Reading {
	at: Instant,
	mapped: Arc<Mutex<Option<Result<(), wgpu::BufferAsyncError>>>>,
	/// Whether the capture stopped being live since, so what it read is
	/// from before and judged neither dark nor good.
	stale: bool,
}

/// Watches one capture's pictures for going dark.
pub(crate) struct Probe {
	device: wgpu::Device,
	queue: wgpu::Queue,
	tiles: wgpu::Buffer,
	readback: wgpu::Buffer,
	/// A copy of the picture last read, since the capture's own texture is
	/// drawn over by its next picture before the read comes back.
	read: Option<wgpu::Texture>,
	/// The newest picture judged not dark, bound to draw.
	good: Option<(wgpu::Texture, Picture)>,
	reading: Option<Reading>,
	/// When the last read was taken.
	last: Option<Instant>,
	/// Whether a picture arrived since the last read.
	changed: bool,
	/// When the first of the dark pictures read since the last good one was
	/// taken.
	dark: Option<Instant>,
	/// The [generation](crate::source::Feed::generation) of the feed it
	/// watches, once it has watched one.
	feed: Option<u64>,
}

impl Probe {
	pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
		let tiles = device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("moq-scene dark tiles"),
			size: BYTES,
			usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
			mapped_at_creation: false,
		});
		let readback = device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("moq-scene dark tiles"),
			size: BYTES,
			usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
			mapped_at_creation: false,
		});
		Self {
			device: device.clone(),
			queue: queue.clone(),
			tiles,
			readback,
			read: None,
			good: None,
			reading: None,
			last: None,
			changed: true,
			dark: None,
			feed: None,
		}
	}

	/// Watch the feed of `generation`: one restarted, such as opened or
	/// picked again, has a new one, and nothing read from the feed before
	/// may be held for it, as that may be another window.
	pub fn follow(&mut self, generation: u64) {
		if self.feed.replace(generation).is_some_and(|was| was != generation) {
			self.forget();
		}
	}

	/// Forget the last good picture and what is being read, and that it is
	/// dark.
	fn forget(&mut self) {
		self.dark = None;
		self.good = None;
		if let Some(reading) = &mut self.reading {
			reading.stale = true;
		}
	}

	/// Note that the capture's texture holds a new picture.
	pub fn changed(&mut self) {
		self.changed = true;
	}

	/// Judge the read that came back, if one did, and read `picture`, the
	/// capture's newest, if it is new and the last read was at least
	/// [`INTERVAL`] ago. `picture` needs `COPY_SRC`.
	///
	/// A capture that is not `live`, such as one reopening, is not dark, and
	/// forgets its last good picture and what it is reading, so nothing from
	/// before it reopens is drawn for it after: a picked window picked again
	/// may be another window.
	pub fn step(&mut self, prober: &Prober, effects: &Effects, picture: &wgpu::Texture, live: bool, now: Instant) {
		self.judge(effects);
		if !live {
			self.forget();
			return;
		}
		let due = self
			.last
			.is_none_or(|last| now.saturating_duration_since(last) >= INTERVAL);
		if self.reading.is_none() && self.changed && due {
			self.start(prober, picture, now);
		}
	}

	/// How long the capture has been dark, once that is at least [`DARK`].
	pub fn dark(&self, now: Instant) -> Option<Duration> {
		let since = now.saturating_duration_since(self.dark?);
		(since >= DARK).then_some(since)
	}

	/// The picture to draw in place of the capture's own: the last good one,
	/// while the capture has been dark for less than [`DARK`].
	pub fn held(&self, now: Instant) -> Option<&Picture> {
		let since = now.saturating_duration_since(self.dark?);
		let (_, picture) = self.good.as_ref()?;
		(since < DARK).then_some(picture)
	}

	/// Copy `picture` and reduce the copy to tiles, read back once the GPU is
	/// done.
	fn start(&mut self, prober: &Prober, picture: &wgpu::Texture, now: Instant) {
		let size = picture.size();
		let read = match self.read.take() {
			Some(read) if read.size() == size && read.format() == picture.format() => read,
			_ => self.device.create_texture(&wgpu::TextureDescriptor {
				label: Some("moq-scene dark read"),
				size,
				mip_level_count: 1,
				sample_count: 1,
				dimension: wgpu::TextureDimension::D2,
				format: picture.format(),
				usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
				view_formats: &[],
			}),
		};
		let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("moq-scene dark"),
			layout: &prober.layout,
			entries: &[
				wgpu::BindGroupEntry {
					binding: 0,
					resource: wgpu::BindingResource::TextureView(&read.create_view(&Default::default())),
				},
				wgpu::BindGroupEntry {
					binding: 1,
					resource: self.tiles.as_entire_binding(),
				},
			],
		});
		let mut encoder = self.device.create_command_encoder(&Default::default());
		encoder.copy_texture_to_texture(picture.as_image_copy(), read.as_image_copy(), size);
		{
			let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
				label: Some("moq-scene dark"),
				timestamp_writes: None,
			});
			pass.set_pipeline(&prober.pipeline);
			pass.set_bind_group(0, &bind, &[]);
			pass.dispatch_workgroups(TILES.0, TILES.1, 1);
		}
		encoder.copy_buffer_to_buffer(&self.tiles, 0, &self.readback, 0, BYTES);

		self.queue.submit([encoder.finish()]);
		let mapped = Arc::new(Mutex::new(None));
		let done = mapped.clone();
		// Called from a later submit or poll, once the GPU is done.
		self.readback.map_async(wgpu::MapMode::Read, .., move |result| {
			*done.lock().unwrap() = Some(result);
		});
		self.read = Some(read);
		self.reading = Some(Reading {
			at: now,
			mapped,
			stale: false,
		});
		self.last = Some(now);
		self.changed = false;
	}

	/// Judge the read under way, if it came back: a dark picture starts or
	/// continues the dark, and any other ends it and becomes the good one.
	fn judge(&mut self, effects: &Effects) {
		let Some(reading) = &self.reading else { return };
		let Some(mapped) = reading.mapped.lock().unwrap().take() else {
			return;
		};
		let (at, stale) = (reading.at, reading.stale);
		self.reading = None;
		let dark = mapped.map_err(|err| err.to_string()).and_then(|()| {
			let dark = match self.readback.slice(..).get_mapped_range() {
				Ok(view) => Ok(judge(&view)),
				Err(err) => Err(format!("{err:?}")),
			};
			self.readback.unmap();
			dark
		});
		let dark = match dark {
			Ok(_) if stale => return,
			Ok(dark) => dark,
			Err(err) => {
				tracing::warn!(%err, "cannot read a capture back to see if it went dark");
				return;
			}
		};
		if dark {
			self.dark.get_or_insert(at);
			return;
		}
		self.dark = None;
		let read = self.read.take().expect("a read has its copy");
		let picture = effects.picture(&read);
		// The good picture before becomes the next read's copy.
		self.read = self.good.replace((read, picture)).map(|(texture, _)| texture);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// `tiles` tiles of one color, its brightest channel as given.
	fn plain(rgb: [u8; 3], brightest: u8) -> Vec<u8> {
		[rgb[0], rgb[1], rgb[2], brightest].repeat((TILES.0 * TILES.1) as usize)
	}

	#[test]
	fn black_is_dark() {
		assert!(judge(&plain([0, 0, 0], 0)));
	}

	/// Studio-range black, or a camera's noise behind its shutter.
	#[test]
	fn near_black_with_noise_is_dark() {
		let mut tiles = plain([16, 16, 16], 30);
		tiles[4..8].copy_from_slice(&[20, 18, 19, 38]);
		assert!(judge(&tiles));
	}

	/// A pointer the capture draws over the black lights a few tiles, and
	/// leaves it dark.
	#[test]
	fn a_pointer_on_black_is_dark() {
		let mut tiles = plain([0, 0, 0], 0);
		for tile in [100, 101, 164, 165] {
			tiles[tile * 4..tile * 4 + 4].copy_from_slice(&[40, 40, 40, 255]);
		}
		assert!(judge(&tiles));
	}

	/// A little more lit than a pointer is not dark, however small each
	/// light: a line of text, or the stars of a night sky.
	#[test]
	fn a_few_lit_pixels_are_not_dark() {
		let mut tiles = plain([0, 0, 0], 0);
		// Each tile's mean barely moves, but its brightest does.
		for tile in [100, 300, 500, 700, 900] {
			tiles[tile * 4..tile * 4 + 4].copy_from_slice(&[1, 1, 1, 255]);
		}
		assert!(!judge(&tiles));
	}

	/// A dim page with a line of dim text: every texel is dark, but not one
	/// color.
	#[test]
	fn dim_text_is_not_dark() {
		let mut tiles = plain([10, 10, 10], 12);
		for tile in 64..96 {
			tiles[tile * 4..tile * 4 + 4].copy_from_slice(&[24, 24, 24, 40]);
		}
		assert!(!judge(&tiles));
	}

	#[test]
	fn a_bright_color_is_not_dark() {
		assert!(!judge(&plain([200, 200, 200], 200)));
	}

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

	/// A capture's 1080p picture, as the compositor's renderer leaves it.
	struct Capture {
		queue: wgpu::Queue,
		texture: wgpu::Texture,
	}

	impl Capture {
		const SIZE: (u32, u32) = (1920, 1080);

		fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
			let texture = device.create_texture(&wgpu::TextureDescriptor {
				label: None,
				size: wgpu::Extent3d {
					width: Self::SIZE.0,
					height: Self::SIZE.1,
					depth_or_array_layers: 1,
				},
				mip_level_count: 1,
				sample_count: 1,
				dimension: wgpu::TextureDimension::D2,
				format: crate::FORMAT,
				usage: wgpu::TextureUsages::TEXTURE_BINDING
					| wgpu::TextureUsages::COPY_SRC
					| wgpu::TextureUsages::COPY_DST,
				view_formats: &[],
			});
			Self {
				queue: queue.clone(),
				texture,
			}
		}

		/// Fill it with `rgb`, and a one-texel line of `line` across row 500.
		fn show(&self, rgb: [u8; 3], line: Option<[u8; 3]>) {
			let (width, height) = Self::SIZE;
			let mut pixels = [rgb[0], rgb[1], rgb[2], 255].repeat((width * height) as usize);
			if let Some(line) = line {
				let row = (500 * width * 4) as usize;
				for pixel in pixels[row..row + (width * 4) as usize].as_chunks_mut::<4>().0 {
					pixel[..3].copy_from_slice(&line);
				}
			}
			self.queue.write_texture(
				self.texture.as_image_copy(),
				&pixels,
				wgpu::TexelCopyBufferLayout {
					offset: 0,
					bytes_per_row: Some(width * 4),
					rows_per_image: None,
				},
				self.texture.size(),
			);
		}
	}

	/// Read `capture` at `at`, and judge the read once the GPU is done.
	fn read(
		probe: &mut Probe,
		prober: &Prober,
		effects: &Effects,
		device: &wgpu::Device,
		capture: &Capture,
		at: Instant,
	) {
		probe.changed();
		probe.step(prober, effects, &capture.texture, true, at);
		assert!(probe.reading.is_some(), "a read started");
		device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
		probe.step(prober, effects, &capture.texture, true, at);
		assert!(probe.reading.is_none(), "the read came back");
	}

	/// The shader reads every texel: one line of light across a black 1080p
	/// picture is enough to keep it from going dark.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just render-sw"]
	async fn every_texel_counts() {
		let (device, queue) = gpu().await;
		let effects = Effects::new(&device, &queue, 64, 64);
		let prober = Prober::new(&device);
		let capture = Capture::new(&device, &queue);
		let mut probe = Probe::new(&device, &queue);
		let start = kio::clock::now();
		let at = |offset: Duration| start.saturating_add(offset);

		capture.show([0, 0, 0], Some([255, 255, 255]));
		read(&mut probe, &prober, &effects, &device, &capture, at(Duration::ZERO));
		assert_eq!(probe.dark, None);

		capture.show([0, 0, 0], None);
		read(&mut probe, &prober, &effects, &device, &capture, at(INTERVAL));
		assert_eq!(probe.dark, Some(at(INTERVAL)));
	}

	/// A moment of dark shows the last good picture in its place; dark for
	/// [`DARK`], the capture counts as gone dark and shows as it is; and a
	/// picture again ends it.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just render-sw"]
	async fn holds_the_last_good_picture_over_a_moment_of_dark() {
		let (device, queue) = gpu().await;
		let effects = Effects::new(&device, &queue, 64, 64);
		let prober = Prober::new(&device);
		let capture = Capture::new(&device, &queue);
		let mut probe = Probe::new(&device, &queue);
		let start = kio::clock::now();
		let at = |offset: Duration| start.saturating_add(offset);

		// Dark from its first picture: nothing good to hold.
		capture.show([0, 0, 0], None);
		read(&mut probe, &prober, &effects, &device, &capture, at(Duration::ZERO));
		assert!(probe.held(at(Duration::ZERO)).is_none());

		let lit = INTERVAL;
		capture.show([90, 120, 200], None);
		read(&mut probe, &prober, &effects, &device, &capture, at(lit));
		assert!(probe.held(at(lit)).is_none() && probe.dark(at(lit)).is_none());

		let dark = lit + INTERVAL;
		capture.show([4, 4, 4], None);
		read(&mut probe, &prober, &effects, &device, &capture, at(dark));
		let almost = dark + DARK - Duration::from_millis(1);
		assert!(probe.held(at(almost)).is_some(), "the last good picture is held");
		assert_eq!(probe.dark(at(almost)), None);
		assert!(probe.held(at(dark + DARK)).is_none(), "the dark shows");
		assert_eq!(
			probe.dark(at(dark + DARK + Duration::from_secs(9))),
			Some(DARK + Duration::from_secs(9))
		);

		// Not read again without a new picture.
		probe.step(&prober, &effects, &capture.texture, true, at(dark + INTERVAL / 2));
		assert!(probe.reading.is_none());

		// A capture that stops being live, such as one reopening, is not dark,
		// and forgets its last good picture: reopened dark, it holds nothing
		// from before, which may be another window.
		probe.step(&prober, &effects, &capture.texture, false, at(dark + DARK));
		assert_eq!(probe.dark(at(dark + DARK)), None);
		let reopened = dark + DARK + INTERVAL;
		capture.show([0, 0, 0], None);
		read(&mut probe, &prober, &effects, &device, &capture, at(reopened));
		assert!(probe.held(at(reopened)).is_none(), "nothing from before it reopened");

		let back = reopened + INTERVAL;
		capture.show([90, 120, 200], None);
		read(&mut probe, &prober, &effects, &device, &capture, at(back));
		assert_eq!(probe.dark(at(back + DARK)), None);

		// A read under way as it stops being live is from before, so it never
		// becomes the picture held after.
		let before = back + INTERVAL;
		capture.show([200, 40, 40], None);
		probe.changed();
		probe.step(&prober, &effects, &capture.texture, true, at(before));
		assert!(probe.reading.is_some(), "a read started");
		probe.step(&prober, &effects, &capture.texture, false, at(before));
		device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
		probe.step(&prober, &effects, &capture.texture, true, at(before));
		assert!(probe.reading.is_none(), "the read came back");
		let after = before + INTERVAL;
		capture.show([0, 0, 0], None);
		read(&mut probe, &prober, &effects, &device, &capture, at(after));
		assert!(probe.held(at(after)).is_none(), "the read from before is not held");
	}

	/// A feed started again, live before the compositor saw it stop, holds
	/// nothing read from the feed before: not its last good picture, nor a
	/// read under way.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just render-sw"]
	async fn a_feed_started_again_holds_nothing_from_before() {
		let (device, queue) = gpu().await;
		let effects = Effects::new(&device, &queue, 64, 64);
		let prober = Prober::new(&device);
		let capture = Capture::new(&device, &queue);
		let mut probe = Probe::new(&device, &queue);
		let start = kio::clock::now();
		let at = |offset: Duration| start.saturating_add(offset);

		probe.follow(1);
		capture.show([90, 120, 200], None);
		read(&mut probe, &prober, &effects, &device, &capture, at(Duration::ZERO));
		probe.follow(1);
		capture.show([0, 0, 0], None);
		read(&mut probe, &prober, &effects, &device, &capture, at(INTERVAL));
		assert!(
			probe.held(at(INTERVAL)).is_some(),
			"the same feed holds its last good picture"
		);

		probe.follow(2);
		assert!(probe.held(at(INTERVAL)).is_none(), "nothing from the feed before");
		assert_eq!(probe.dark(at(INTERVAL + DARK)), None);

		// A read under way as the feed starts again is from before.
		let before = INTERVAL * 2;
		capture.show([200, 40, 40], None);
		probe.changed();
		probe.step(&prober, &effects, &capture.texture, true, at(before));
		assert!(probe.reading.is_some(), "a read started");
		probe.follow(3);
		device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
		let after = before + INTERVAL;
		capture.show([0, 0, 0], None);
		read(&mut probe, &prober, &effects, &device, &capture, at(after));
		assert!(probe.held(at(after)).is_none(), "the read from before is not held");
	}
}
