//! Guided setup: on first launch, find this machine's devices, the encoders
//! that work on it, and the uplink to the relay, then propose a scene and a
//! canvas in plain language before the studio starts.
//!
//! Every probe that fails is shown with its reason, and the proposal says what
//! it assumed instead.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Context;
use egui::Ui;
use moq_net::bandwidth::Rate;
use moq_scene::{Canvas, Collection, Draft, Edit, History, Item, Rect, Scene, wgpu};

use crate::autosave::{self, Autosave};
use crate::devices::{self, Listing, Pick};
use crate::engine::{self, Engine, Launch};
use crate::health::rate;
use crate::uplink;

/// The canvases setup can propose, best first. Each encodes at its own default
/// bitrate, so a smaller one is what fits a slower uplink.
const CANVASES: [Canvas; 5] = [
	canvas(1920, 1080, 60),
	canvas(1920, 1080, 30),
	canvas(1280, 720, 60),
	canvas(1280, 720, 30),
	canvas(640, 360, 30),
];

const fn canvas(width: u32, height: u32, framerate: u32) -> Canvas {
	Canvas {
		width,
		height,
		framerate,
	}
}

/// The share of the measured uplink the program may use: as much as keeps the
/// Health panel's headroom grade healthy, so a proposal is not flagged the
/// moment it goes live. The rest absorbs the estimate's error and whatever else
/// shares the connection.
fn headroom() -> f64 {
	1.0 / hang::health::Thresholds::default().transport.headroom.degraded
}

/// Frames timed per canvas once its encoder opens: enough to average out one
/// slow frame, few enough that setup stays quick.
const BURST: u32 = 15;

/// The share of each frame's period that copying and encoding it may take, so
/// the encoder still keeps up while sources and the compositor share the machine.
const PACE: f64 = 0.75;

/// How far the timed picture pans each frame, so the encoder searches motion as
/// it does for live content instead of skipping a still picture.
const PAN: u32 = 4;

/// The scene setup proposes.
const SCENE: &str = "Main";

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
	fn total(&self) -> Rate {
		Rate::from_bps(self.bitrate.as_bps() + engine::AUDIO_BITRATE.as_bps())
	}

	/// Whether the encoder keeps up with `canvas` in real time, within [`PACE`].
	fn keeps_up(&self, canvas: Canvas) -> bool {
		self.frame <= budget(canvas)
	}

	/// How many times its [`budget`] a frame of `canvas` took, scaled to compare
	/// canvases without floats.
	fn load(&self, canvas: Canvas) -> u128 {
		self.frame.as_nanos() * u128::from(canvas.framerate)
	}
}

/// The time a frame of `canvas` may take to copy and encode.
fn budget(canvas: Canvas) -> Duration {
	(Duration::from_secs(1) / canvas.framerate).mul_f64(PACE)
}

/// Each of [`CANVASES`], with how it encodes or why it cannot.
pub type Encoders = Vec<(Canvas, Result<Encoding, String>)>;

/// The canvases setup may propose: those whose encoder keeps up, or the one
/// closest to keeping up when none does, best first.
fn candidates(encoders: &Encoders) -> Vec<(Canvas, &Encoding)> {
	let encodable = encoders
		.iter()
		.filter_map(|(canvas, result)| Some((*canvas, result.as_ref().ok()?)));
	let fast: Vec<_> = encodable
		.clone()
		.filter(|(canvas, encoding)| encoding.keeps_up(*canvas))
		.collect();
	match fast.is_empty() {
		true => encodable
			.min_by_key(|(canvas, encoding)| encoding.load(*canvas))
			.into_iter()
			.collect(),
		false => fast,
	}
}

/// Open the exporter the program would for each canvas, the same probe that
/// picks its encoder at startup, and time a burst through what it settled on.
async fn probe_encoders(gpu: engine::Gpu) -> Encoders {
	let picture = Picture::new(&gpu);
	let mut probed = Vec::new();
	for canvas in CANVASES {
		let result = probe_encoder(&gpu, &picture, canvas)
			.await
			.map_err(|err| format!("{err:#}"));
		probed.push((canvas, result));
	}
	probed
}

async fn probe_encoder(gpu: &engine::Gpu, picture: &Picture, canvas: Canvas) -> anyhow::Result<Encoding> {
	use moq_video::render::export::{Exporter, Route};

	let mut config = moq_video::encode::Config::new(canvas.width, canvas.height, canvas.framerate());
	let (mut exporter, rendition) = Exporter::new(&gpu.device, &gpu.queue, &mut config).await?;
	let bitrate = rendition.bitrate.context("the encoder reports no bitrate")?;
	let encoder = match &config.kind {
		moq_video::encode::Kind::Named(name) => name.clone(),
		other => format!("{other:?}"),
	};

	// The program's own path: a canvas-sized texture copied through the
	// exporter, waited on, and encoded, as its encode thread does.
	let started = kio::clock::now();
	let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
		label: Some("setup burst"),
		size: extent(canvas.width, canvas.height),
		mip_level_count: 1,
		sample_count: 1,
		dimension: wgpu::TextureDimension::D2,
		format: wgpu::TextureFormat::Rgba8Unorm,
		usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
		view_formats: &[],
	});
	let mut session = moq_video::encode::Encoder::new(&config)?;
	let period = Duration::from_secs(1) / canvas.framerate;
	let frame = time(budget(canvas), async |index| {
		picture.pan(gpu, &target, index);
		let timestamp = moq_net::Timestamp::try_from(period * index)?;
		let pending = match exporter.copy(&target, timestamp)? {
			Some(pending) => pending,
			// Fed back to back, as the program never feeds it, a pipelined
			// encoder can hold every slot; drained, it gives them back.
			None => {
				session.flush()?;
				exporter
					.copy(&target, timestamp)?
					.context("the encoder holds every frame")?
			}
		};
		session.encode(&pending.wait()?)?;
		// So the time covers the frames a pipelined encoder still holds.
		if index == BURST {
			session.flush()?;
		}
		Ok(())
	})
	.await?;
	tracing::info!(
		canvas = describe(canvas),
		encoder,
		?frame,
		burst = ?started.elapsed(),
		"timed the encoder"
	);

	Ok(Encoding {
		encoder,
		gpu: exporter.route() == Route::Gpu,
		bitrate: Rate::from_bps(bitrate),
		frame,
	})
}

/// How long a frame takes through `encode`, on average over [`BURST`] frames,
/// calling it with each index from 0 through [`BURST`].
///
/// Frame 0 is not timed: it is a keyframe, and the encoder's first frame.
/// The burst stops as soon as it is over `budget` a frame in total, since its
/// average can no longer come in under it, so a slow encoder costs no more
/// time than a fast one.
async fn time(budget: Duration, mut encode: impl AsyncFnMut(u32) -> anyhow::Result<()>) -> anyhow::Result<Duration> {
	encode(0).await?;
	let started = kio::clock::now();
	for index in 1..=BURST {
		encode(index).await?;
		let elapsed = started.elapsed();
		if elapsed > budget * BURST {
			return Ok(elapsed / index);
		}
	}
	Ok(started.elapsed() / BURST)
}

/// A picture as large as the largest canvas plus a burst's panning, uploaded
/// once and copied from for every canvas.
struct Picture(wgpu::Texture);

impl Picture {
	fn new(gpu: &engine::Gpu) -> Self {
		let width = CANVASES.iter().map(|canvas| canvas.width).max().unwrap_or_default() + PAN * BURST;
		let height = CANVASES.iter().map(|canvas| canvas.height).max().unwrap_or_default();
		let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
			label: Some("setup picture"),
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
		Self(texture)
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

/// The canvas to propose: the best candidate whose program fits the uplink's
/// [`headroom`], or the cheapest candidate when none fits. Without a
/// measurement it is the default canvas when that is a candidate.
fn pick(encoders: &Encoders, uplink: Option<Rate>) -> Canvas {
	let candidates = candidates(encoders);
	let Some(uplink) = uplink else {
		let default = Canvas::default();
		return candidates
			.iter()
			.map(|(canvas, _)| *canvas)
			.find(|canvas| *canvas == default)
			.or_else(|| candidates.first().map(|(canvas, _)| *canvas))
			.unwrap_or(default);
	};
	let budget = uplink.scaled(headroom());
	candidates
		.iter()
		.find(|(_, encoding)| encoding.total() <= budget)
		.or_else(|| candidates.iter().min_by_key(|(_, encoding)| encoding.total()))
		.map(|(canvas, _)| *canvas)
		.unwrap_or_default()
}

/// Why `canvas` suits this machine, in plain words.
fn explain(canvas: Canvas, encoders: &Encoders, uplink: &Result<Rate, String>) -> String {
	let name = describe(canvas);
	let encoding = match encoders.iter().find(|(probed, _)| *probed == canvas) {
		Some((_, Ok(encoding))) => encoding,
		Some((_, Err(err))) => {
			return format!("{name} cannot be encoded on this machine, so publishing it fails: {err}");
		}
		None => return format!("{name} was not probed."),
	};
	let route = match encoding.gpu {
		true => "on the GPU",
		false => "from a copy read back to the CPU each frame",
	};
	let mut text = format!(
		"{name}: {} encodes it {route} at about {}, {} with audio.",
		encoding.encoder,
		rate(encoding.bitrate),
		rate(encoding.total())
	);
	if !encoding.keeps_up(canvas) {
		text.push_str(&format!(
			" A frame took {}, more than the {} keeping up at {} fps allows, so expect dropped frames.",
			millis(encoding.frame),
			millis(budget(canvas)),
			canvas.framerate
		));
	}
	let uplink = match uplink {
		Ok(uplink) => *uplink,
		Err(_) => {
			text.push_str(" The uplink was not measured, so check that it carries that.");
			return text;
		}
	};
	let share = encoding.total().as_bps() as f64 / uplink.as_bps().max(1) as f64;
	text.push_str(&format!(
		" That is {}% of the {} uplink measured to the relay",
		(share * 100.0).round(),
		rate(uplink)
	));
	match share <= headroom() {
		true => text.push('.'),
		false => text.push_str(&format!(
			", more than the {}% it should use. Expect dropped frames.",
			(headroom() * 100.0).round()
		)),
	}
	text
}

fn describe(canvas: Canvas) -> String {
	format!("{}x{} at {} fps", canvas.width, canvas.height, canvas.framerate)
}

fn millis(duration: Duration) -> String {
	format!("{:.1} ms", duration.as_secs_f64() * 1e3)
}

/// The devices and canvas the creator settles on, each picked by index into
/// the listing.
#[derive(Clone, Debug, Default, PartialEq)]
struct Choices {
	display: Option<usize>,
	camera: Option<usize>,
	audio: Option<usize>,
	canvas: Canvas,
}

/// The first of each kind of device, on `canvas`.
fn propose(listing: &Listing, canvas: Canvas) -> Choices {
	let first = |devices: &Result<Vec<Pick>, String>| devices.as_ref().ok().filter(|d| !d.is_empty()).map(|_| 0);
	Choices {
		display: first(&listing.displays),
		camera: first(&listing.cameras),
		audio: first(&listing.audio),
		canvas,
	}
}

/// What setup starts the studio with: a new collection on the chosen canvas
/// and, unless `empty`, the proposal as its first edit, so the history holds it
/// and undoing it leaves the empty collection.
fn loaded(listing: &Listing, choices: &Choices, empty: bool) -> anyhow::Result<autosave::Loaded> {
	let mut collection = Collection::new();
	collection.canvas = choices.canvas;
	let mut history = History::new();
	if !empty {
		let edit = proposal(&collection, listing, choices)?;
		collection.apply(&edit)?;
		history.record(edit, None);
	}
	Ok(autosave::Loaded {
		collection,
		history,
		dropped: 0,
	})
}

/// The edit `choices` describe to the new `collection`: its scene becomes
/// [`SCENE`], with the display filling the canvas and the camera over its
/// corner (or filling the canvas alone), and the audio input is added.
fn proposal(collection: &Collection, listing: &Listing, choices: &Choices) -> Result<Edit, moq_scene::Error> {
	let chosen = |devices: &Result<Vec<Pick>, String>, index: Option<usize>| {
		devices.as_ref().ok().and_then(|devices| devices.get(index?)).cloned()
	};
	let display = chosen(&listing.displays, choices.display);
	let camera = chosen(&listing.cameras, choices.camera);
	let audio = chosen(&listing.audio, choices.audio);

	let mut draft = collection.draft(format!("Set up scene `{SCENE}`"));
	let mut scene = Scene::new(SCENE);
	let fill = Rect::fill(&choices.canvas);
	let corner = corner(&choices.canvas);
	let placed = [
		(display.as_ref(), fill),
		(camera.as_ref(), if display.is_some() { corner } else { fill }),
	];
	for (pick, rect) in placed {
		let Some(pick) = pick else { continue };
		let name = add(&mut draft, pick)?;
		scene.items.push(Item::new(name, rect));
	}
	if let Some(pick) = &audio {
		add(&mut draft, pick)?;
	}
	draft.set_scene(0, scene)?;
	draft.finish()
}

/// Add `pick` under its device name, numbered past any source already called that.
fn add(draft: &mut Draft, pick: &Pick) -> Result<String, moq_scene::Error> {
	let name = draft.collection().unused_name(&pick.name);
	draft.set_source(&name, Some(pick.source.clone()))?;
	Ok(name)
}

/// A quarter of the canvas wide in its bottom-right corner, 16:9 like most
/// cameras, inset by a fortieth of the width.
fn corner(canvas: &Canvas) -> Rect {
	let width = canvas.width as f32 / 4.0;
	let height = width * 9.0 / 16.0;
	let inset = canvas.width as f32 / 40.0;
	Rect {
		x: canvas.width as f32 - width - inset,
		y: canvas.height as f32 - height - inset,
		width,
		height,
	}
}

/// A result being worked out in the background.
enum Task<T> {
	Running(mpsc::Receiver<T>),
	Done(T),
}

impl<T: Send + 'static> Task<T> {
	/// Run `task` in `runtime` on a thread of its own, since encoder probes and
	/// the engine's start block while they open encoders. Not on the runtime's
	/// blocking pool, which dropping the runtime waits out, so a probe that never
	/// ends, such as one waiting on a camera permission prompt, cannot hold the
	/// studio open once its window closes.
	fn spawn<F, Fut>(runtime: &tokio::runtime::Handle, task: F) -> std::io::Result<Self>
	where
		F: FnOnce() -> Fut + Send + 'static,
		Fut: Future<Output = T>,
	{
		let (send, recv) = mpsc::channel();
		let runtime = runtime.clone();
		std::thread::Builder::new().name("setup".into()).spawn(move || {
			let _ = send.send(runtime.block_on(task()));
		})?;
		Ok(Self::Running(recv))
	}

	/// The result, once it arrived.
	///
	/// Panics if the task did, rather than waiting forever on a result that
	/// will never come.
	fn poll(&mut self) -> Option<&T> {
		if let Self::Running(recv) = self {
			match recv.try_recv() {
				Ok(done) => *self = Self::Done(done),
				Err(mpsc::TryRecvError::Empty) => {}
				Err(mpsc::TryRecvError::Disconnected) => panic!("a setup task stopped without a result"),
			}
		}
		match self {
			Self::Running(_) => None,
			Self::Done(done) => Some(done),
		}
	}
}

/// The setup screen.
pub struct Setup {
	runtime: tokio::runtime::Handle,
	launch: Launch,
	/// The scene file, which does not exist until setup starts the studio.
	path: PathBuf,
	devices: Task<Listing>,
	encoders: Task<Encoders>,
	/// Started once the encoders are known, since they set what to probe up to.
	uplink: Option<Task<Result<Rate, String>>>,
	/// Filled in once every probe is done, then edited by the creator.
	choices: Option<Choices>,
	starting: Option<Task<anyhow::Result<Arc<Engine>>>>,
	/// Why the last start failed.
	error: Option<String>,
}

impl Setup {
	/// Start probing for `launch`, to create the scene file at `path`.
	pub fn new(launch: Launch, path: PathBuf, runtime: tokio::runtime::Handle) -> std::io::Result<Self> {
		let devices = Task::spawn(&runtime, devices::list)?;
		let gpu = launch.gpu.clone();
		let encoders = Task::spawn(&runtime, move || probe_encoders(gpu))?;
		Ok(Self {
			runtime,
			launch,
			path,
			devices,
			encoders,
			uplink: None,
			choices: None,
			starting: None,
			error: None,
		})
	}

	/// Draw the screen; the engine once the creator has started the studio.
	pub fn ui(&mut self, ui: &mut Ui) -> Option<Arc<Engine>> {
		self.advance();
		if self.choices.is_none() || self.starting.is_some() {
			ui.ctx().request_repaint_after(Duration::from_millis(100));
		}
		if let Some(starting) = &mut self.starting
			&& starting.poll().is_some()
		{
			let Some(Task::Done(result)) = self.starting.take() else {
				unreachable!("polled done")
			};
			match result {
				Ok(engine) => return Some(engine),
				Err(err) => self.error = Some(format!("{err:#}")),
			}
		}

		let mut start = None;
		egui::CentralPanel::default().show(ui, |ui| {
			egui::ScrollArea::vertical().show(ui, |ui| {
				ui.heading("Set up MoQ Studio");
				ui.label(
					"Setup checks this machine and proposes a first scene and canvas. Change anything here, \
					 or later: sources and scenes in their panels, the canvas in the scene file.",
				);
				ui.separator();
				self.findings(ui);
				ui.separator();
				start = self.proposal(ui);
			});
		});
		if let Some(empty) = start {
			self.start(empty);
		}
		None
	}

	/// Collect finished probes, start the uplink probe once the encoders are
	/// known, and propose once everything is in.
	fn advance(&mut self) {
		self.devices.poll();
		if self.uplink.is_none()
			&& let Some(encoders) = self.encoders.poll()
		{
			let ceiling = candidates(encoders)
				.into_iter()
				.map(|(_, encoding)| encoding.total())
				.max()
				.map(|top| top.scaled(1.0 / headroom()));
			// The first relay's, which the stream's bandwidth follows.
			let connect = self
				.launch
				.relays
				.first()
				.map(|relay| (relay.client.clone(), relay.url.clone(), self.launch.broadcast.clone()));
			self.uplink = Some(match (connect, ceiling) {
				(None, _) => Task::Done(Err("no relay is configured; start with --connect to measure it".into())),
				(Some(_), None) => Task::Done(Err("nothing encodes on this machine, so nothing would be sent".into())),
				(Some((client, url, path)), Some(ceiling)) => {
					let measured = Task::spawn(&self.runtime, move || async move {
						uplink::measure(&client, url, &path, ceiling, uplink::WINDOW)
							.await
							.map_err(|err| format!("{err:#}"))
					});
					measured.unwrap_or_else(|err| Task::Done(Err(format!("cannot start the probe: {err}"))))
				}
			});
		}
		if let Some(uplink) = &mut self.uplink {
			uplink.poll();
		}
		if self.choices.is_none()
			&& let (Task::Done(listing), Task::Done(encoders), Some(Task::Done(uplink))) =
				(&self.devices, &self.encoders, &self.uplink)
		{
			let canvas = pick(encoders, uplink.as_ref().ok().copied());
			self.choices = Some(propose(listing, canvas));
		}
	}

	fn findings(&mut self, ui: &mut Ui) {
		ui.strong("This machine");
		egui::Grid::new("findings").num_columns(2).show(ui, |ui| {
			ui.label("Devices");
			match &mut self.devices {
				Task::Running(_) => {
					ui.spinner();
				}
				Task::Done(listing) => {
					ui.vertical(|ui| {
						for (kind, devices) in listing.kinds() {
							match devices {
								Ok(devices) => ui.label(format!("{kind}: {}", devices.len())),
								Err(err) => ui.colored_label(ui.visuals().error_fg_color, format!("{kind}: {err}")),
							};
						}
					});
				}
			}
			ui.end_row();

			ui.label("Encoders");
			match &self.encoders {
				Task::Running(_) => {
					ui.horizontal(|ui| {
						ui.spinner();
						ui.label("timing a short burst through each canvas's encoder");
					});
				}
				Task::Done(encoders) => {
					ui.vertical(|ui| {
						for (canvas, result) in encoders {
							match result {
								Ok(encoding) => {
									let text = format!(
										"{}: {}{}, {}, {} a frame",
										describe(*canvas),
										encoding.encoder,
										if encoding.gpu { " on the GPU" } else { " from the CPU" },
										rate(encoding.bitrate),
										millis(encoding.frame),
									);
									match encoding.keeps_up(*canvas) {
										true => ui.label(text),
										false => ui.colored_label(
											ui.visuals().warn_fg_color,
											format!("{text}, too slow to keep up"),
										),
									}
								}
								Err(err) => ui.colored_label(
									ui.visuals().error_fg_color,
									format!("{}: {err}", describe(*canvas)),
								),
							};
						}
					});
				}
			}
			ui.end_row();

			ui.label("Uplink");
			match &self.uplink {
				None => {
					ui.weak("measured once the encoders are known");
				}
				Some(Task::Running(_)) => {
					ui.horizontal(|ui| {
						ui.spinner();
						ui.label(format!(
							"publishing a probe to the relay for about {} s",
							uplink::WINDOW.as_secs()
						));
					});
				}
				Some(Task::Done(Ok(uplink))) => {
					ui.label(format!("{} to the relay", rate(*uplink)));
				}
				Some(Task::Done(Err(err))) => {
					ui.colored_label(ui.visuals().error_fg_color, format!("not measured: {err}"));
				}
			}
			ui.end_row();
		});
	}

	/// Draw the proposal; `Some(empty)` once the creator starts the studio, with
	/// `empty` when they turned the proposal down.
	fn proposal(&mut self, ui: &mut Ui) -> Option<bool> {
		let (Some(choices), Task::Done(listing), Task::Done(encoders), Some(Task::Done(uplink))) =
			(&mut self.choices, &self.devices, &self.encoders, &self.uplink)
		else {
			ui.label("Proposing once every check is done.");
			return None;
		};

		let idle = self.starting.is_none();
		ui.strong(format!("Proposed scene: {SCENE}"));
		ui.add_enabled_ui(idle, |ui| {
			egui::Grid::new("proposal").num_columns(3).show(ui, |ui| {
				let has_display = device(ui, "Display", "display", &listing.displays, &mut choices.display);
				ui.label(match (has_display, choices.camera.is_some()) {
					(true, _) => "Fills the canvas, so viewers see your screen.",
					(false, true) => "No display, so the camera fills the canvas.",
					(false, false) => "No display.",
				});
				ui.end_row();

				let has_camera = device(ui, "Camera", "camera", &listing.cameras, &mut choices.camera);
				ui.label(match (has_camera, choices.display.is_some()) {
					(true, true) => "Sits in the bottom-right corner, a quarter of the canvas wide, over the display.",
					(true, false) => "Fills the canvas.",
					(false, _) => "No camera.",
				});
				ui.end_row();

				let has_audio = device(ui, "Audio", "audio", &listing.audio, &mut choices.audio);
				ui.label(match has_audio {
					true => "Mixed into the program. Set its level in the Audio panel.",
					false => "No input, so the program is silent unless a source carries sound.",
				});
				ui.end_row();

				ui.label("Canvas");
				egui::ComboBox::from_id_salt("canvas")
					.selected_text(describe(choices.canvas))
					.show_ui(ui, |ui| {
						for (canvas, _) in encoders {
							ui.selectable_value(&mut choices.canvas, *canvas, describe(*canvas));
						}
					});
				ui.label(explain(choices.canvas, encoders, uplink));
				ui.end_row();
			});
		});

		if let Some(error) = &self.error {
			ui.colored_label(ui.visuals().error_fg_color, error);
		}
		if !idle {
			ui.spinner();
			return None;
		}
		let mut start = None;
		ui.horizontal(|ui| {
			if ui.button("Start with this scene").clicked() {
				start = Some(false);
			}
			if ui.button("Start with an empty scene").clicked() {
				start = Some(true);
			}
		});
		start
	}

	/// Start the studio with the proposal, or with no sources on the chosen
	/// canvas when `empty`. Either way its autosave creates the scene file at
	/// once.
	fn start(&mut self, empty: bool) {
		let (Task::Done(listing), Some(choices)) = (&self.devices, &self.choices) else {
			return;
		};
		let loaded = match loaded(listing, choices, empty) {
			Ok(loaded) => loaded,
			Err(err) => {
				self.error = Some(format!("{err:#}"));
				return;
			}
		};
		// Kept, so a start that fails can be retried with other choices.
		let launch = self.launch.clone();
		let path = self.path.clone();
		let starting = Task::spawn(&self.runtime, move || async move {
			// Setup proposes only for a missing file, so one that appeared since,
			// such as another studio's, is opened by a restart, not overwritten.
			// The autosave's first save refuses one that appears after this.
			let (autosave, _) = Autosave::open(path)?;
			anyhow::ensure!(
				!autosave.exists(),
				"{} was created during setup; restart the studio to open it",
				autosave.path().display()
			);
			Engine::start(launch, autosave, loaded).await
		});
		match starting {
			Ok(starting) => {
				self.error = None;
				self.starting = Some(starting);
			}
			Err(err) => self.error = Some(format!("cannot start the studio: {err}")),
		}
	}
}

/// A row picking one of `devices` or none, or saying why there is none to
/// pick. Whether one is picked.
fn device(ui: &mut Ui, label: &str, id: &str, devices: &Result<Vec<Pick>, String>, choice: &mut Option<usize>) -> bool {
	ui.label(label);
	match devices {
		Ok(devices) if !devices.is_empty() => {
			let selected = choice
				.and_then(|index| devices.get(index))
				.map_or("None", |pick| pick.name.as_str());
			egui::ComboBox::from_id_salt(id)
				.selected_text(selected)
				.show_ui(ui, |ui| {
					ui.selectable_value(choice, None, "None");
					for (index, pick) in devices.iter().enumerate() {
						ui.selectable_value(choice, Some(index), &pick.name);
					}
				});
		}
		Ok(_) => {
			ui.weak("none found");
		}
		Err(err) => {
			ui.colored_label(ui.visuals().error_fg_color, "cannot list")
				.on_hover_text(err);
		}
	}
	choice.is_some()
}

#[cfg(test)]
mod tests {
	use moq_scene::Source;

	use super::*;

	/// [`CANVASES`] each encoding at its default bitrate and keeping up, or
	/// failing where `None`.
	fn encoders(kbps: [Option<u64>; 5]) -> Encoders {
		CANVASES
			.into_iter()
			.zip(kbps)
			.map(|(canvas, kbps)| {
				let result = kbps
					.map(|kbps| Encoding {
						encoder: "test".into(),
						gpu: true,
						bitrate: Rate::from_kbps(kbps),
						frame: Duration::from_millis(1),
					})
					.ok_or_else(|| "refused".to_string());
				(canvas, result)
			})
			.collect()
	}

	/// [`encoders`] at their default bitrates, each frame taking `millis`.
	fn timed(millis: [u64; 5]) -> Encoders {
		let mut encoders = encoders(DEFAULTS);
		for ((_, result), millis) in encoders.iter_mut().zip(millis) {
			result.as_mut().unwrap().frame = Duration::from_millis(millis);
		}
		encoders
	}

	const DEFAULTS: [Option<u64>; 5] = [Some(8709), Some(4354), Some(3870), Some(1935), Some(484)];

	#[test]
	fn picks_the_best_canvas_within_the_headroom() {
		let encoders = encoders(DEFAULTS);
		// 4.354 + 0.128 Mbps is just inside two thirds of 6.8 Mbps, where the
		// Health panel's headroom grade turns degraded.
		assert_eq!(pick(&encoders, Some(Rate::from_kbps(6800))), CANVASES[1]);
		assert_eq!(pick(&encoders, Some(Rate::from_kbps(6700))), CANVASES[2]);
		assert_eq!(pick(&encoders, Some(Rate::from_mbps(100))), CANVASES[0]);
	}

	#[test]
	fn skips_canvases_whose_encoder_cannot_keep_up() {
		// 60 fps leaves 12.5 ms a frame, and 30 fps 25 ms.
		let encoders = timed([20, 20, 12, 12, 5]);
		assert_eq!(pick(&encoders, Some(Rate::from_mbps(100))), CANVASES[1]);
		assert_eq!(pick(&encoders, None), Canvas::default());
		let encoders = timed([20, 30, 12, 12, 5]);
		assert_eq!(pick(&encoders, Some(Rate::from_mbps(100))), CANVASES[2]);
		assert_eq!(pick(&encoders, None), CANVASES[2], "the default does not keep up");
	}

	#[test]
	fn proposes_the_closest_to_keeping_up_when_none_does() {
		let encoders = timed([100, 80, 60, 40, 30]);
		assert_eq!(pick(&encoders, Some(Rate::from_mbps(100))), CANVASES[4]);
		assert_eq!(pick(&encoders, None), CANVASES[4]);
	}

	/// Times `time` over an encoder whose frames take `first` and then `rest`
	/// on the paused clock, with how many frames it was handed.
	async fn burst(budget: Duration, first: Duration, rest: Duration) -> (Duration, u32) {
		let mut calls = 0;
		let frame = time(budget, async |index| {
			calls += 1;
			kio::clock::sleep(if index == 0 { first } else { rest }).await;
			Ok(())
		})
		.await
		.unwrap();
		(frame, calls)
	}

	#[tokio::test(start_paused = true)]
	async fn times_every_frame_but_the_first() {
		let budget = Duration::from_millis(25);
		let (frame, calls) = burst(budget, Duration::from_secs(1), Duration::from_millis(10)).await;
		assert_eq!(frame, Duration::from_millis(10));
		assert_eq!(calls, BURST + 1);
	}

	/// A slow encoder is cut off once it cannot average under the budget, so
	/// it takes no longer than one that keeps up.
	#[tokio::test(start_paused = true)]
	async fn stops_a_burst_that_falls_behind() {
		let budget = Duration::from_millis(25);
		let started = kio::clock::now();
		let (frame, calls) = burst(budget, Duration::ZERO, Duration::from_millis(50)).await;
		assert_eq!(frame, Duration::from_millis(50));
		// Over the 375 ms the whole burst may take, at the eighth timed frame.
		assert_eq!(calls, 9);
		assert!(started.elapsed() <= budget * BURST + Duration::from_millis(50));
	}

	/// The burst runs on a real device and encoder, through the program's own
	/// copy path.
	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "needs a Vulkan device and an H.264 encoder; run by just render-sw"]
	async fn times_every_canvas() {
		let gpu = engine::Gpu::new().await.expect("a GPU");
		for (canvas, result) in probe_encoders(gpu).await {
			let encoding = result.unwrap_or_else(|err| panic!("{}: {err}", describe(canvas)));
			assert!(encoding.frame > Duration::ZERO, "{}", describe(canvas));
		}
	}

	#[test]
	fn falls_back_to_the_cheapest_canvas_that_encodes() {
		let encoders = encoders([Some(8709), Some(4354), Some(3870), Some(1935), None]);
		assert_eq!(pick(&encoders, Some(Rate::from_kbps(300))), CANVASES[3]);
	}

	#[test]
	fn skips_canvases_that_do_not_encode() {
		let encoders = encoders([None, Some(4354), Some(3870), Some(1935), Some(484)]);
		assert_eq!(pick(&encoders, Some(Rate::from_mbps(100))), CANVASES[1]);
	}

	#[test]
	fn keeps_the_default_canvas_without_a_measurement() {
		assert_eq!(pick(&encoders(DEFAULTS), None), Canvas::default());
		assert_eq!(
			pick(&encoders([None; 5]), Some(Rate::from_mbps(100))),
			Canvas::default()
		);
	}

	fn listing(cameras: &[&str], displays: &[&str], audio: &[&str]) -> Listing {
		let picks = |names: &[&str], source: fn(String) -> Source| {
			Ok(names
				.iter()
				.map(|name| Pick {
					name: name.to_string(),
					source: source(format!("{name}-id")),
				})
				.collect())
		};
		Listing {
			cameras: picks(cameras, |id| Source::Camera { id: Some(id) }),
			displays: picks(displays, |id| Source::Display { id: Some(id) }),
			windows: Ok(Vec::new()),
			audio: picks(audio, |id| Source::Microphone { id: Some(id) }),
		}
	}

	#[test]
	fn the_camera_sits_over_the_display() {
		let listing = listing(&["Webcam"], &["Screen"], &["Mic"]);
		let choices = propose(&listing, CANVASES[3]);
		let autosave::Loaded {
			mut collection,
			mut history,
			..
		} = loaded(&listing, &choices, false).unwrap();
		collection.to_toml().expect("a valid collection");

		let items = &collection.scene(SCENE).expect("the scene").items;
		let placed: Vec<_> = items.iter().map(|item| item.source.as_str()).collect();
		assert_eq!(placed, ["Screen", "Webcam"], "back to front");
		assert_eq!(items[0].rect, Rect::fill(&CANVASES[3]));
		let corner = items[1].rect;
		assert!(
			corner.x > 640.0 && corner.y > 360.0,
			"in the bottom-right quarter: {corner:?}"
		);
		assert!(corner.x + corner.width < 1280.0 && corner.y + corner.height < 720.0);
		assert!(collection.sources.contains_key("Mic"), "the input is a source");

		// One edit, which undoes to what starting empty opens.
		assert_eq!(history.entries().len(), 1);
		history.undo(&mut collection).unwrap();
		assert_eq!(collection, loaded(&listing, &choices, true).unwrap().collection);
		assert!(collection.sources.is_empty());
		assert_eq!(collection.canvas, CANVASES[3]);
	}

	#[test]
	fn the_camera_alone_fills_the_canvas() {
		let listing = listing(&["Webcam"], &["Screen"], &[]);
		let mut choices = propose(&listing, Canvas::default());
		choices.display = None;
		let collection = loaded(&listing, &choices, false).unwrap().collection;
		let items = &collection.scene(SCENE).expect("the scene").items;
		assert_eq!(items.len(), 1);
		assert_eq!(items[0].rect, Rect::fill(&Canvas::default()));
	}

	#[test]
	fn devices_sharing_a_name_get_their_own_sources() {
		let listing = listing(&["Built-in"], &["Built-in"], &["Built-in"]);
		let collection = loaded(&listing, &propose(&listing, Canvas::default()), false)
			.unwrap()
			.collection;
		let names: Vec<_> = collection.sources.keys().map(String::as_str).collect();
		assert_eq!(names, ["Built-in", "Built-in 2", "Built-in 3"]);
		collection.to_toml().expect("a valid collection");
	}
}
