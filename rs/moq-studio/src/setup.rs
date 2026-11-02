//! Guided setup: on first launch, find this machine's devices, the encoders
//! that work on it, and the uplink to the relay, then propose a scene and a
//! canvas in plain language before the studio starts.
//!
//! Every probe that fails is shown with its reason, and the proposal says what
//! it assumed instead.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use egui::Ui;
use moq_net::bandwidth::Rate;
use moq_scene::{Canvas, Collection, Item, Rect, Scene};

use crate::devices::{self, Listing, Pick};
use crate::engine::{self, Engine, Launch};
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

/// The share of the measured uplink the program may use. The rest absorbs the
/// estimate's error and whatever else shares the connection.
const HEADROOM: f64 = 0.75;

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
}

impl Encoding {
	/// What the program sends: this video plus the audio.
	fn total(&self) -> Rate {
		Rate::from_bps(self.bitrate.as_bps() + engine::AUDIO_BITRATE.as_bps())
	}
}

/// Each of [`CANVASES`], with how it encodes or why it cannot.
pub type Encoders = Vec<(Canvas, Result<Encoding, String>)>;

/// Open the exporter the program would for each canvas, the same probe that
/// picks its encoder at startup, and keep what it settled on.
async fn probe_encoders(gpu: engine::Gpu) -> Encoders {
	let mut probed = Vec::new();
	for canvas in CANVASES {
		let mut config = moq_video::encode::Config::new(canvas.width, canvas.height, canvas.framerate());
		let result = moq_video::render::export::Exporter::new(&gpu.device, &gpu.queue, &mut config)
			.await
			.map_err(|err| err.to_string())
			.and_then(|(exporter, rendition)| {
				let encoder = match &config.kind {
					moq_video::encode::Kind::Named(name) => name.clone(),
					other => format!("{other:?}"),
				};
				let bitrate = rendition.bitrate.ok_or("the encoder reports no bitrate")?;
				Ok(Encoding {
					encoder,
					gpu: exporter.route() == moq_video::render::export::Route::Gpu,
					bitrate: Rate::from_bps(bitrate),
				})
			});
		probed.push((canvas, result));
	}
	probed
}

/// The canvas to propose: the best one that encodes and whose program fits the
/// uplink's [`HEADROOM`], or the cheapest that encodes when none fits. Without a
/// measurement it is the default canvas when that encodes.
fn pick(encoders: &Encoders, uplink: Option<Rate>) -> Canvas {
	let encodable = || {
		encoders
			.iter()
			.filter_map(|(canvas, result)| Some((*canvas, result.as_ref().ok()?)))
	};
	let Some(uplink) = uplink else {
		let default = Canvas::default();
		return encodable()
			.map(|(canvas, _)| canvas)
			.find(|canvas| *canvas == default)
			.or_else(|| encodable().next().map(|(canvas, _)| canvas))
			.unwrap_or(default);
	};
	let budget = uplink.scaled(HEADROOM);
	encodable()
		.find(|(_, encoding)| encoding.total() <= budget)
		.or_else(|| encodable().min_by_key(|(_, encoding)| encoding.total()))
		.map(|(canvas, _)| canvas)
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
		true => "reading it on the GPU",
		false => "from a copy read back to the CPU each frame",
	};
	let cost = format!(
		"{} encodes it with {route} at about {}, {} with audio",
		encoding.encoder,
		mbps(encoding.bitrate),
		mbps(encoding.total())
	);
	let uplink = match uplink {
		Ok(uplink) => *uplink,
		Err(_) => return format!("{name}: {cost}. The uplink was not measured, so check that it carries that."),
	};
	let share = encoding.total().as_bps() as f64 / uplink.as_bps().max(1) as f64;
	let percent = (share * 100.0).round();
	if share <= HEADROOM {
		format!(
			"{name}: {cost}, {percent}% of the {} uplink measured to the relay.",
			mbps(uplink)
		)
	} else {
		format!(
			"{name}: {cost}, {percent}% of the {} uplink measured to the relay, more than the {}% it should use. Expect dropped frames.",
			mbps(uplink),
			(HEADROOM * 100.0).round()
		)
	}
}

fn describe(canvas: Canvas) -> String {
	format!("{}x{} at {} fps", canvas.width, canvas.height, canvas.framerate)
}

fn mbps(rate: Rate) -> String {
	format!("{:.1} Mbps", rate.as_bps() as f64 / 1e6)
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

/// The collection `choices` describe: the display filling the canvas, the
/// camera over its corner (or filling the canvas alone), and the audio input.
fn collection(listing: &Listing, choices: &Choices) -> Collection {
	let chosen = |devices: &Result<Vec<Pick>, String>, index: Option<usize>| {
		devices.as_ref().ok().and_then(|devices| devices.get(index?)).cloned()
	};
	let display = chosen(&listing.displays, choices.display);
	let camera = chosen(&listing.cameras, choices.camera);
	let audio = chosen(&listing.audio, choices.audio);

	let mut collection = Collection::new();
	collection.canvas = choices.canvas;
	let mut scene = Scene::new(SCENE);
	let fill = Rect::fill(&choices.canvas);
	let corner = corner(&choices.canvas);
	let placed = [
		(display.as_ref(), fill),
		(camera.as_ref(), if display.is_some() { corner } else { fill }),
	];
	for (pick, rect) in placed {
		let Some(pick) = pick else { continue };
		let name = add(&mut collection, pick);
		scene.items.push(Item {
			source: name,
			rect,
			visible: true,
		});
	}
	if let Some(pick) = &audio {
		add(&mut collection, pick);
	}
	collection.scenes = vec![scene];
	collection
}

/// Add `pick` under its device name, numbered past any source already called that.
fn add(collection: &mut Collection, pick: &Pick) -> String {
	let name = std::iter::once(pick.name.clone())
		.chain((2..).map(|n| format!("{} {n}", pick.name)))
		.find(|name| !collection.sources.contains_key(name))
		.expect("an unused name");
	collection.sources.insert(name.clone(), pick.source.clone());
	name
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

/// What the studio starts with once setup is done: its engine, and the line for
/// its status bar.
pub struct Started {
	pub engine: Arc<Engine>,
	pub message: Option<Result<String, String>>,
}

/// A result being worked out in the background.
enum Task<T> {
	Running(mpsc::Receiver<T>),
	Done(T),
}

impl<T: Send + 'static> Task<T> {
	/// Run `task` on a blocking thread of `runtime`, since encoder probes and the
	/// engine's start block while they open encoders.
	fn spawn<F, Fut>(runtime: &tokio::runtime::Handle, task: F) -> Self
	where
		F: FnOnce() -> Fut + Send + 'static,
		Fut: Future<Output = T>,
	{
		let (send, recv) = mpsc::channel();
		let handle = runtime.clone();
		runtime.spawn_blocking(move || {
			let _ = send.send(handle.block_on(task()));
		});
		Self::Running(recv)
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
	devices: Task<Listing>,
	encoders: Task<Encoders>,
	/// Started once the encoders are known, since they set what to probe up to.
	uplink: Option<Task<Result<Rate, String>>>,
	/// Filled in once every probe is done, then edited by the creator.
	choices: Option<Choices>,
	starting: Option<Task<anyhow::Result<Started>>>,
	/// Why the last start failed.
	error: Option<String>,
}

impl Setup {
	/// Start probing for `launch`.
	pub fn new(launch: Launch, runtime: tokio::runtime::Handle) -> Self {
		let devices = Task::spawn(&runtime, devices::list);
		let gpu = launch.gpu.clone();
		let encoders = Task::spawn(&runtime, move || probe_encoders(gpu));
		Self {
			runtime,
			launch,
			devices,
			encoders,
			uplink: None,
			choices: None,
			starting: None,
			error: None,
		}
	}

	/// Draw the screen; what the studio starts with once the creator starts it.
	pub fn ui(&mut self, ui: &mut Ui) -> Option<Started> {
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
				Ok(started) => return Some(started),
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
			let ceiling = encoders
				.iter()
				.filter_map(|(_, result)| result.as_ref().ok())
				.map(Encoding::total)
				.max()
				.map(|top| top.scaled(1.0 / HEADROOM));
			let connect = self
				.launch
				.connect
				.clone()
				.map(|(client, url)| (client, url, self.launch.broadcast.clone()));
			self.uplink = Some(match (connect, ceiling) {
				(None, _) => Task::Done(Err("no relay is configured; start with --connect to measure it".into())),
				(Some(_), None) => Task::Done(Err("nothing encodes on this machine, so nothing would be sent".into())),
				(Some((client, url, path)), Some(ceiling)) => Task::spawn(&self.runtime, move || async move {
					uplink::measure(&client, url, &path, ceiling, uplink::WINDOW)
						.await
						.map_err(|err| format!("{err:#}"))
				}),
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
					ui.spinner();
				}
				Task::Done(encoders) => {
					ui.vertical(|ui| {
						for (canvas, result) in encoders {
							match result {
								Ok(encoding) => ui.label(format!(
									"{}: {}{}, {}",
									describe(*canvas),
									encoding.encoder,
									if encoding.gpu { " on the GPU" } else { " from the CPU" },
									mbps(encoding.bitrate)
								)),
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
				Some(Task::Done(Ok(rate))) => {
					ui.label(format!("{} to the relay", mbps(*rate)));
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

	/// Start the studio with the proposal, writing it to the scene file, or
	/// with an empty collection when `empty`, which writes nothing yet.
	fn start(&mut self, empty: bool) {
		let (Task::Done(listing), Some(choices)) = (&self.devices, &self.choices) else {
			return;
		};
		let proposed = (!empty).then(|| collection(listing, choices));
		// Kept, so a start that fails can be retried with other choices.
		let launch = self.launch.clone();
		self.error = None;
		self.starting = Some(Task::spawn(&self.runtime, move || async move {
			let path = launch.path.clone();
			let engine = Engine::start(launch, proposed.clone().unwrap_or_default()).await?;
			let message = proposed.map(|_| {
				let saved = engine.state.lock().unwrap().save();
				saved
					.map(|()| format!("set up; saved {}", path.display()))
					.map_err(|err| format!("{err:#}"))
			});
			Ok(Started { engine, message })
		}));
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

	/// [`CANVASES`] each encoding at its default bitrate, or failing where `None`.
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
					})
					.ok_or_else(|| "refused".to_string());
				(canvas, result)
			})
			.collect()
	}

	const DEFAULTS: [Option<u64>; 5] = [Some(8709), Some(4354), Some(3870), Some(1935), Some(484)];

	#[test]
	fn picks_the_best_canvas_within_the_headroom() {
		let encoders = encoders(DEFAULTS);
		// 4.354 + 0.128 Mbps is just inside three quarters of 6 Mbps.
		assert_eq!(pick(&encoders, Some(Rate::from_mbps(6))), CANVASES[1]);
		assert_eq!(pick(&encoders, Some(Rate::from_kbps(5900))), CANVASES[2]);
		assert_eq!(pick(&encoders, Some(Rate::from_mbps(100))), CANVASES[0]);
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
		let collection = collection(&listing, &choices);
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
	}

	#[test]
	fn the_camera_alone_fills_the_canvas() {
		let listing = listing(&["Webcam"], &["Screen"], &[]);
		let mut choices = propose(&listing, Canvas::default());
		choices.display = None;
		let collection = collection(&listing, &choices);
		let items = &collection.scene(SCENE).expect("the scene").items;
		assert_eq!(items.len(), 1);
		assert_eq!(items[0].rect, Rect::fill(&Canvas::default()));
	}

	#[test]
	fn devices_sharing_a_name_get_their_own_sources() {
		let listing = listing(&["Built-in"], &["Built-in"], &["Built-in"]);
		let collection = collection(&listing, &propose(&listing, Canvas::default()));
		let names: Vec<_> = collection.sources.keys().map(String::as_str).collect();
		assert_eq!(names, ["Built-in", "Built-in 2", "Built-in 3"]);
		collection.to_toml().expect("a valid collection");
	}
}
