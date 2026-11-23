//! Timing the encoders: how a canvas encodes on this machine, through the
//! program's own copy path, for a new show's canvas and the go-live sheet's
//! check.

use std::sync::mpsc;
use std::time::Duration;

use anyhow::Context;
use moq_net::bandwidth::Rate;
use moq_scene::Canvas;

use crate::engine;

/// The canvas a new show starts on when a hardware encoder keeps up with it:
/// 1080p at 60 fps. Otherwise the default, 1080p at 30.
const SMOOTH: Canvas = Canvas {
	width: 1920,
	height: 1080,
	framerate: 60,
};

/// The share of the measured uplink the program may use: as much as keeps the
/// Health panel's headroom grade healthy, so a stream the go-live check passes
/// is not flagged the moment it goes live. The rest absorbs the estimate's error and whatever else
/// shares the connection.
pub(crate) fn headroom() -> f64 {
	1.0 / hang::health::Thresholds::default().transport.headroom.degraded
}

/// Frames timed per canvas once its encoder opens: enough to average out one
/// slow frame, few enough that a check stays quick.
const BURST: u32 = 15;

/// The share of each frame's period that copying and encoding it may take, so
/// the encoder still keeps up while sources and the compositor share the machine.
const PACE: f64 = 0.75;

/// How far the timed picture pans each frame, so the encoder searches motion as
/// it does for live content instead of skipping a still picture.
const PAN: u32 = 4;

/// How one canvas encodes on this machine, as the program's own encoder choice
/// settles it.
#[derive(Clone, Debug, PartialEq)]
pub struct Encoding {
	/// The encoder backend, such as `nvenc` or `openh264`.
	pub encoder: String,
	/// Whether it reads the canvas on the GPU rather than a copy on the CPU.
	pub gpu: bool,
	/// The video bitrate it encodes at.
	pub bitrate: Rate,
	/// How long copying and encoding a frame took, on average over a burst.
	pub frame: Duration,
}

impl Encoding {
	/// What the program sends: this video plus the audio.
	pub(crate) fn total(&self) -> Rate {
		Rate::from_bps(self.bitrate.as_bps() + engine::AUDIO_BITRATE.as_bps())
	}

	/// Whether the encoder keeps up with `canvas` in real time, within [`PACE`].
	pub(crate) fn keeps_up(&self, canvas: Canvas) -> bool {
		self.frame <= budget(canvas)
	}
}

/// The time a frame of `canvas` may take to copy and encode.
fn budget(canvas: Canvas) -> Duration {
	(Duration::from_secs(1) / canvas.framerate).mul_f64(PACE)
}

/// The canvas a new show starts on: [`SMOOTH`] when a hardware encoder keeps
/// up with it, timed once through the program's copy path, and otherwise the
/// default. A software encoder stays at the default even when it keeps up on
/// a still burst, since a live program also shares its CPU with the sources.
pub(crate) async fn first_canvas(gpu: engine::Gpu) -> Canvas {
	match probe(gpu, &[SMOOTH]).await {
		Ok(encodings) if smooth(&encodings) => SMOOTH,
		Ok(encodings) => {
			tracing::info!(encoder = encodings[0].encoder, "starting the show at 30 fps");
			Canvas::default()
		}
		Err(err) => {
			tracing::warn!(%err, "cannot time the encoder; starting the show at 30 fps");
			Canvas::default()
		}
	}
}

/// Whether `encodings`, of [`SMOOTH`] alone, come from a hardware encoder
/// that keeps up with it.
fn smooth(encodings: &[Encoding]) -> bool {
	encodings[0].encoder != "openh264" && keeps_up(&[SMOOTH], encodings)
}

/// How `canvases` encode on this machine at once, as going live checks them,
/// each timed while the others run: the pictures a show sends together, such
/// as its landscape and portrait ones. The result follows the order given.
///
/// Fails when any cannot open its encoder, as a hardware encoder at its
/// session cap cannot, since going live would not send it either.
pub(crate) async fn probe(gpu: engine::Gpu, canvases: &[Canvas]) -> Result<Vec<Encoding>, String> {
	let picture = Picture::new(&gpu, canvases)?;
	probe_together(&gpu, &picture, canvases)
		.await
		.map_err(|err| format!("{err:#}"))
}

/// Whether every encode in `encodings`, one per canvas of `canvases`, keeps up
/// with its canvas while the others run.
pub(crate) fn keeps_up(canvases: &[Canvas], encodings: &[Encoding]) -> bool {
	canvases
		.iter()
		.zip(encodings)
		.all(|(canvas, encoding)| encoding.keeps_up(*canvas))
}

/// What `encodings` send together, each a broadcast with its own audio.
pub(crate) fn total(encodings: &[Encoding]) -> Rate {
	Rate::from_bps(encodings.iter().map(|encoding| encoding.total().as_bps()).sum())
}

/// One encode being timed: the exporter the program would open for a canvas,
/// and the texture each frame is copied into. Its encoder opens on the thread
/// that runs it, as the program's does, since an encoder stays on one.
struct Lane {
	canvas: Canvas,
	exporter: moq_video::render::export::Exporter,
	config: moq_video::encode::Config,
	target: wgpu::Texture,
	encoder: String,
	bitrate: Rate,
}

impl Lane {
	/// Open the exporter the program would for `canvas`, which settles its
	/// encoder; the encode session opens on the burst's first frame.
	async fn open(gpu: &engine::Gpu, canvas: Canvas) -> anyhow::Result<Self> {
		let config = moq_video::encode::Config::new(canvas.width, canvas.height, canvas.framerate());
		let (exporter, config, rendition) = moq_video::render::export::Exporter::new(&gpu.device, &gpu.queue, &config)
			.await
			.with_context(|| format!("cannot encode {}", describe(canvas)))?;
		let bitrate = rendition.bitrate.context("the encoder reports no bitrate")?;
		let encoder = match &config.kind {
			moq_video::encode::Kind::Named(name) => name.clone(),
			other => format!("{other:?}"),
		};
		// The program's own path: a canvas-sized texture copied through the
		// exporter, waited on, and encoded, as its encode thread does.
		let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
			label: Some("encoder burst"),
			size: extent(canvas.width, canvas.height),
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: wgpu::TextureFormat::Rgba8Unorm,
			usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
			view_formats: &[],
		});
		Ok(Self {
			canvas,
			exporter,
			config,
			target,
			encoder,
			bitrate: Rate::from_bps(bitrate),
		})
	}

	/// Encode frame `index` of a burst, and how long it took from `handed`,
	/// when it went to every encode, opening the encoder into `session` on the
	/// first. A thread that was slow to start counts against the encode.
	fn frame(
		&mut self,
		session: &mut Option<moq_video::encode::Encoder>,
		gpu: &engine::Gpu,
		picture: &Picture,
		index: u32,
		handed: kio::clock::Instant,
	) -> anyhow::Result<Duration> {
		let session = match session {
			Some(session) => session,
			None => session.insert(moq_video::encode::Encoder::new(&self.config)?),
		};
		picture.pan(gpu, &self.target, index);
		let period = Duration::from_secs(1) / self.canvas.framerate;
		let timestamp = moq_net::Timestamp::try_from(period * index)?;
		let pending = match self.exporter.copy(&self.target, timestamp)? {
			Some(pending) => pending,
			// Fed back to back, as the program never feeds it, a pipelined
			// encoder can hold every slot; drained, it gives them back.
			None => {
				session.flush()?;
				self.exporter
					.copy(&self.target, timestamp)?
					.context("the encoder holds every frame")?
			}
		};
		session.encode(&pending.wait()?)?;
		// So the time covers the frames a pipelined encoder still holds.
		if index == BURST {
			session.flush()?;
		}
		Ok(handed.elapsed())
	}

	fn finish(self, frame: Duration) -> Encoding {
		Encoding {
			encoder: self.encoder,
			gpu: self.exporter.route() == moq_video::render::export::Route::Gpu,
			bitrate: self.bitrate,
			frame,
		}
	}
}

/// Open an encoder for each of `canvases` and time a burst through all of them
/// at once: each frame is handed to every encode together, one thread each, as
/// the compositor hands the program's encode threads theirs.
async fn probe_together(gpu: &engine::Gpu, picture: &Picture, canvases: &[Canvas]) -> anyhow::Result<Vec<Encoding>> {
	// Every exporter settles its encoder before any is timed. Each thread opens
	// its session on frame 0 and holds it through the burst, as the program's
	// encode threads do, so a refused one fails the check.
	let mut lanes = Vec::new();
	for canvas in canvases {
		lanes.push(Lane::open(gpu, *canvas).await?);
	}
	let budgets: Vec<Duration> = canvases.iter().map(|canvas| budget(*canvas)).collect();
	let started = kio::clock::now();
	let (frames, lanes) = std::thread::scope(|scope| {
		// A thread the OS refuses fails the check; dropping the feeds already
		// made ends the threads that did start, so the scope can join them.
		let workers = lanes
			.into_iter()
			.map(|mut lane| {
				let (feed, indexes) = mpsc::channel::<(u32, kio::clock::Instant)>();
				let (done, took) = mpsc::channel();
				let thread = std::thread::Builder::new().spawn_scoped(scope, move || {
					let mut session = None;
					let canvas = lane.canvas;
					for (index, handed) in indexes {
						let result = lane
							.frame(&mut session, gpu, picture, index, handed)
							.with_context(|| format!("cannot encode {}", describe(canvas)));
						if done.send(result).is_err() {
							break;
						}
					}
					lane
				})?;
				Ok((feed, took, thread))
			})
			.collect::<std::io::Result<Vec<_>>>();
		let workers = match workers {
			Ok(workers) => workers,
			Err(err) => {
				return (
					Err(anyhow::Error::new(err).context("cannot start an encode thread")),
					Vec::new(),
				);
			}
		};
		let frames = time(&budgets, |index| {
			let handed = kio::clock::now();
			for (feed, ..) in &workers {
				feed.send((index, handed)).context("an encode thread ended")?;
			}
			workers
				.iter()
				.map(|(_, took, _)| took.recv().context("an encode thread ended").and_then(|result| result))
				.collect()
		});
		// Ends each thread, whose lane comes back for its encoder's name.
		let lanes: Vec<Lane> = workers
			.into_iter()
			.map(|(feed, took, thread)| {
				drop((feed, took));
				thread.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))
			})
			.collect();
		(frames, lanes)
	});
	let encodings: Vec<Encoding> = lanes
		.into_iter()
		.zip(frames?)
		.map(|(lane, frame)| lane.finish(frame))
		.collect();
	for (canvas, encoding) in canvases.iter().zip(&encodings) {
		tracing::info!(
			canvas = describe(*canvas),
			encoder = encoding.encoder,
			frame = ?encoding.frame,
			together = encodings.len(),
			burst = ?started.elapsed(),
			"timed the encoder"
		);
	}
	Ok(encodings)
}

/// How long a frame of each encode takes through `encode`, on average over
/// [`BURST`] frames, calling it with each index from 0 through [`BURST`]; it
/// returns each encode's own time for that frame.
///
/// Frame 0 is not timed: it is a keyframe, and the encoder's first frame.
/// The burst stops as soon as any encode is over its budget a frame in total,
/// since its average can no longer come in under it, so a slow encoder costs
/// no more time than a fast one.
fn time(
	budgets: &[Duration],
	mut encode: impl FnMut(u32) -> anyhow::Result<Vec<Duration>>,
) -> anyhow::Result<Vec<Duration>> {
	encode(0)?;
	let mut totals = vec![Duration::ZERO; budgets.len()];
	for index in 1..=BURST {
		let frames = encode(index)?;
		anyhow::ensure!(frames.len() == totals.len(), "an encode went untimed");
		for (total, frame) in totals.iter_mut().zip(frames) {
			*total += frame;
		}
		if totals
			.iter()
			.zip(budgets)
			.any(|(total, budget)| *total > *budget * BURST)
		{
			return Ok(totals.into_iter().map(|total| total / index).collect());
		}
	}
	Ok(totals.into_iter().map(|total| total / BURST).collect())
}

/// A picture as large as the largest canvas it serves plus a burst's panning,
/// uploaded once and copied from for each of them.
struct Picture(wgpu::Texture);

impl Picture {
	/// Fails when the panning room takes it past what the GPU can hold, as a
	/// canvas at the GPU's own limit would, rather than let the GPU panic.
	fn new(gpu: &engine::Gpu, canvases: &[Canvas]) -> Result<Self, String> {
		let width = canvases.iter().map(|canvas| canvas.width).max().unwrap_or_default() + PAN * BURST;
		let height = canvases.iter().map(|canvas| canvas.height).max().unwrap_or_default();
		let limit = gpu.device.limits().max_texture_dimension_2d;
		if width > limit || height > limit {
			return Err(format!(
				"the GPU holds pictures up to {limit} px a side, too small to time a {width}x{height} burst"
			));
		}
		let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
			label: Some("encoder picture"),
			size: extent(width, height),
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: wgpu::TextureFormat::Rgba8Unorm,
			usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
			view_formats: &[],
		});
		// Gradients crossed with a pattern: detail to search, unlike a flat fill,
		// without the cost of noise, which no camera or screen produces.
		let pixels: Vec<u8> = (0..height)
			.flat_map(|y| (0..width).flat_map(move |x| [x as u8, y as u8, (x ^ y) as u8, 0xff]))
			.collect();
		gpu.queue.write_texture(
			texture.as_image_copy(),
			&pixels,
			wgpu::TexelCopyBufferLayout {
				offset: 0,
				bytes_per_row: Some(width * 4),
				rows_per_image: Some(height),
			},
			extent(width, height),
		);
		Ok(Self(texture))
	}

	/// Draw frame `index` of a burst into `target`: the picture, panned [`PAN`]
	/// pixels a frame.
	fn pan(&self, gpu: &engine::Gpu, target: &wgpu::Texture, index: u32) {
		let mut commands = gpu.device.create_command_encoder(&Default::default());
		commands.copy_texture_to_texture(
			wgpu::TexelCopyTextureInfo {
				texture: &self.0,
				mip_level: 0,
				origin: wgpu::Origin3d {
					x: index * PAN,
					y: 0,
					z: 0,
				},
				aspect: wgpu::TextureAspect::All,
			},
			target.as_image_copy(),
			extent(target.width(), target.height()),
		);
		gpu.queue.submit([commands.finish()]);
	}
}

fn extent(width: u32, height: u32) -> wgpu::Extent3d {
	wgpu::Extent3d {
		width,
		height,
		depth_or_array_layers: 1,
	}
}

fn describe(canvas: Canvas) -> String {
	format!("{}x{} at {} fps", canvas.width, canvas.height, canvas.framerate)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// An encoding of 1080p30 at its default bitrate, each frame taking
	/// `millis`.
	fn encoding(encoder: &str, millis: u64) -> Encoding {
		Encoding {
			encoder: encoder.into(),
			gpu: true,
			bitrate: Rate::from_kbps(4354),
			frame: Duration::from_millis(millis),
		}
	}

	/// The portrait picture is a broadcast of its own, audio included.
	#[test]
	fn both_formats_send_the_audio_twice() {
		let encoding = encoding("test", 1);
		assert_eq!(total(std::slice::from_ref(&encoding)), encoding.total());
		assert_eq!(total(&[encoding.clone(), encoding]), Rate::from_kbps(2 * (4354 + 128)));
	}

	/// Each encode keeps up with its own canvas while the others run: the
	/// slower of two is what fails the pair.
	#[test]
	fn both_formats_keep_up_when_each_does() {
		let canvases = [Canvas::default(), Canvas::default().turned()];
		// 30 fps leaves 25 ms a frame.
		assert!(keeps_up(&canvases, &[encoding("test", 20), encoding("test", 25)]));
		assert!(!keeps_up(&canvases, &[encoding("test", 20), encoding("test", 26)]));
		assert!(!keeps_up(&canvases, &[encoding("test", 26), encoding("test", 20)]));
	}

	/// A new show runs at 60 fps only on a hardware encoder that keeps up with
	/// it: 12.5 ms a frame.
	#[test]
	fn sixty_fps_takes_a_hardware_encoder_that_keeps_up() {
		assert!(smooth(&[encoding("nvenc", 12)]));
		assert!(!smooth(&[encoding("nvenc", 13)]), "too slow");
		assert!(!smooth(&[encoding("openh264", 5)]), "software");
	}

	/// Times `time` over an encoder whose frames take `rest`, with how many
	/// frames it was handed; its first takes `first`, which is not timed.
	fn burst(budget: Duration, first: Duration, rest: Duration) -> (Duration, u32) {
		let mut calls = 0;
		let frames = time(&[budget], |index| {
			calls += 1;
			Ok(vec![if index == 0 { first } else { rest }])
		})
		.unwrap();
		(frames[0], calls)
	}

	#[test]
	fn times_every_frame_but_the_first() {
		let budget = Duration::from_millis(25);
		let (frame, calls) = burst(budget, Duration::from_secs(1), Duration::from_millis(10));
		assert_eq!(frame, Duration::from_millis(10));
		assert_eq!(calls, BURST + 1);
	}

	/// A slow encoder is cut off once it cannot average under the budget, so
	/// it takes no longer than one that keeps up.
	#[test]
	fn stops_a_burst_that_falls_behind() {
		let budget = Duration::from_millis(25);
		let (frame, calls) = burst(budget, Duration::ZERO, Duration::from_millis(50));
		assert_eq!(frame, Duration::from_millis(50));
		// Over the 375 ms the whole burst may take, at the eighth timed frame.
		assert_eq!(calls, 9);
	}

	/// Each encode is timed on its own, and the burst stops once any is over
	/// its budget.
	#[test]
	fn times_each_encode_of_a_pair() {
		let budgets = [Duration::from_millis(25), Duration::from_millis(25)];
		let frames = time(&budgets, |_| {
			Ok(vec![Duration::from_millis(10), Duration::from_millis(20)])
		})
		.unwrap();
		assert_eq!(frames, [Duration::from_millis(10), Duration::from_millis(20)]);

		let mut calls = 0;
		let frames = time(&budgets, |_| {
			calls += 1;
			Ok(vec![Duration::from_millis(10), Duration::from_millis(50)])
		})
		.unwrap();
		assert_eq!(frames, [Duration::from_millis(10), Duration::from_millis(50)]);
		assert_eq!(calls, 9, "stopped as the second fell behind");
	}

	/// The burst runs on a real device and encoder, through the program's own
	/// copy path, for a new show's canvases and a portrait show's, taller
	/// than either, as going live checks it, alone and with its landscape
	/// picture beside it.
	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "needs a Vulkan device and an H.264 encoder; run by just render-sw"]
	async fn times_every_canvas() {
		let gpu = engine::Gpu::new(wgpu::Backends::all()).await.expect("a GPU");
		for canvas in [SMOOTH, Canvas::default()] {
			let encodings = probe(gpu.clone(), &[canvas])
				.await
				.unwrap_or_else(|err| panic!("{}: {err}", describe(canvas)));
			assert!(encodings[0].frame > Duration::ZERO, "{}", describe(canvas));
		}
		let landscape = Canvas {
			width: 1280,
			height: 720,
			framerate: 30,
		};
		let portrait = landscape.turned();
		let alone = probe(gpu.clone(), &[portrait])
			.await
			.expect("a portrait canvas encodes");
		assert_eq!(alone.len(), 1);
		let both = probe(gpu.clone(), &[landscape, portrait])
			.await
			.expect("both formats encode");
		assert_eq!(both.len(), 2);
		for encoding in alone.iter().chain(&both) {
			assert!(encoding.frame > Duration::ZERO);
		}
		let first = first_canvas(gpu).await;
		assert!(first == SMOOTH || first == Canvas::default(), "{first:?}");
	}
}
