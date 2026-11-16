//! kixel.live Studio: compose scenes from cameras, screens, images, and other
//! broadcasts on the GPU, and go live over MoQ.
//!
//! The scenes, sources, and compositor live in `moq-scene`; this binary is the
//! window around them and the program clock that publishes the result.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use anyhow::Context;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

mod autosave;
mod controls;
mod devices;
mod engine;
mod feed;
mod fx;
mod guests;
mod health;
mod hotkeys;
mod interact;
mod kit;
mod ladder;
mod mix;
mod past;
mod record;
mod replay;
mod setup;
mod shelf;
#[cfg(test)]
mod shots;
mod snap;
mod ui;
mod uplink;
mod vcam;
mod watch;

#[derive(usage_rs::Cli, Clone)]
#[usage(unknown_flags = "error", args_override_self = false)]
#[usage(name = "moq-studio")]
#[usage(completion, settings, unknown_env(prefix = "MOQ_"))]
pub struct Config {
	/// The scene collection to open, saved as it changes. If it does not exist,
	/// guided setup proposes one and creates it. Defaults to `scenes.toml` in
	/// the studio's data directory, which a launch from the desktop opens.
	#[usage(value_hint = usage_rs::ValueHint::FilePath, extensions("toml"))]
	pub scenes: Option<PathBuf>,

	/// Play a studio broadcast instead, as a viewer sees it: the program with
	/// each source published independently over it. The URL's path names the
	/// broadcast, as `moq play`'s does, such as
	/// `https://relay.example/anon/studio`. A prototype, until `moq play` plays
	/// the layer layout.
	#[usage(long)]
	pub watch: Option<url::Url>,

	/// The broadcast path the program is published at.
	#[usage(long, default = "studio")]
	pub broadcast: String,

	/// Copy and encode the program video at the canvas size only while someone
	/// watches it, as the half size always is. This saves the work while
	/// nobody does, but leaves a late viewer nothing from before they arrived.
	/// Audio is always encoded, and a recording has an encoder of its own.
	#[usage(long)]
	pub on_demand: bool,

	/// The directory recordings and clips are saved in. Defaults to a named
	/// scene file's, or else to the user's videos folder.
	#[usage(long, value_hint = usage_rs::ValueHint::DirPath)]
	pub recordings: Option<PathBuf>,

	/// Guests who join from a web page, appearing as sources.
	#[usage(flatten)]
	pub guests: guests::Config,

	/// The MoQ client configuration. Without `--connect`, nothing is published
	/// and broadcast sources stay offline.
	#[usage(flatten)]
	pub client: moq_tokio::connect::Config,

	/// A second relay to publish the same broadcast to, so a viewer linked to
	/// both resumes from it when the `--connect` relay fails. Both must speak
	/// the one `--connect-version`, which must carry broadcast instances. Its
	/// credential rides its URL's `?jwt=`: `--connect-token` goes to the
	/// `--connect` relay only.
	#[usage(long)]
	pub failover: Option<url::Url>,

	/// QUIC transport tuning (`--quic-*`).
	#[usage(flatten)]
	pub quic: moq_tokio::quic::Config,

	/// The log configuration.
	#[usage(flatten)]
	pub log: moq_tokio::Log,
}

impl Config {
	/// The scene file to open and the directory recordings go to, or `None` to
	/// `--watch` a broadcast, refusing the studio's own flags beside it.
	fn files(&self) -> anyhow::Result<Option<Files>> {
		if self.watch.is_none() {
			return Files::new(self.scenes.clone(), self.recordings.clone()).map(Some);
		}
		anyhow::ensure!(
			self.scenes.is_none(),
			"--watch plays a broadcast, so it opens no scene file"
		);
		anyhow::ensure!(
			self.client.url.is_none() && self.failover.is_none(),
			"--watch names the relay in its URL, so it takes no --connect or --failover"
		);
		anyhow::ensure!(
			self.guests.room.is_none(),
			"--watch publishes nothing, so it has no --room"
		);
		Ok(None)
	}

	/// Refuse a config parsed from released spellings, naming what replaced each.
	fn check_deprecated(&self) -> anyhow::Result<()> {
		let mut deprecated = self.client.deprecated();
		deprecated.extend(self.quic.deprecated());
		anyhow::ensure!(deprecated.is_empty(), "{deprecated}");
		Ok(())
	}

	/// The relays to publish to, the `--connect` one first; none without it.
	fn relays(&self) -> anyhow::Result<Vec<engine::Relay>> {
		let Some(url) = self.client.url.clone() else {
			anyhow::ensure!(self.failover.is_none(), "--failover needs --connect");
			return Ok(Vec::new());
		};
		if self.failover.is_some() {
			self.check_failover()?;
		}
		let client = self.client.clone().init(self.quic.clone())?;
		let mut relays = vec![engine::Relay { client, url }];
		if let Some(url) = self.failover.clone() {
			let mut client = self.client.clone();
			// Its own credentials ride its URL; another operator's never reach it.
			client.tokens.clear();
			let client = client
				.init(self.quic.clone())
				.context("cannot set up the failover relay")?;
			relays.push(engine::Relay { client, url });
		}
		Ok(relays)
	}

	/// Refuse relays a viewer could not move between without a restart: a copy
	/// resumes mid-group only under the same instance, which needs both links on
	/// one protocol version that carries it.
	fn check_failover(&self) -> anyhow::Result<()> {
		let [version] = self.client.version.as_slice() else {
			anyhow::bail!("--failover needs exactly one --connect-version, so both relays negotiate the same protocol");
		};
		anyhow::ensure!(
			matches!(version, moq_net::Version::Lite(lite) if lite.has_instance()),
			"--failover needs a --connect-version with broadcast instances (moq-lite-07-wip today), not {version}, \
			 or a viewer moving between relays restarts the stream rather than resuming it"
		);
		// Each relay's client binds its own socket, so a fixed port would be
		// bound twice.
		anyhow::ensure!(
			self.client.bind.is_none_or(|bind| bind.port() == 0),
			"--failover cannot share a fixed --connect-bind port between the two relays; leave the port 0"
		);
		Ok(())
	}
}

/// Where the studio keeps its own files: the window's settings, and the scene
/// collection when none is named.
fn data_dir() -> Option<PathBuf> {
	eframe::storage_dir("moq-studio")
}

/// The files the studio opens and writes.
#[derive(Debug, PartialEq)]
struct Files {
	scenes: PathBuf,
	recordings: PathBuf,
}

impl Files {
	/// A named scene file keeps its recordings beside it. With none named, as
	/// from the desktop, which passes no arguments, the collection is the one
	/// in the data directory, where nobody looks for a recording, so they go to
	/// the videos folder instead.
	fn new(scenes: Option<PathBuf>, recordings: Option<PathBuf>) -> anyhow::Result<Self> {
		if let Some(scenes) = scenes {
			let recordings = recordings.unwrap_or_else(|| match scenes.parent() {
				Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
				_ => PathBuf::from("."),
			});
			return Ok(Self { scenes, recordings });
		}
		let data = data_dir().context("this system has no data directory to keep scenes in; name a scene file")?;
		let recordings = match recordings {
			Some(dir) => dir,
			None => dirs::video_dir()
				.or_else(std::env::home_dir)
				.context("this system has no videos folder; pass --recordings")?,
		};
		Ok(Self {
			scenes: data.join("scenes.toml"),
			recordings,
		})
	}
}

fn main() -> anyhow::Result<ExitCode> {
	// A browser source's helper, and CEF's subprocesses, are this executable
	// started again.
	if let Some(code) = moq_scene::browser::child() {
		return Ok(code);
	}

	let config = Config::parse();
	config.log.init()?;
	config.check_deprecated()?;
	let Some(Files { scenes, recordings }) = config.files()? else {
		let url = config.watch.clone().expect("checked by files");
		watch::run(config.client.clone(), config.quic.clone(), url)?;
		return Ok(ExitCode::SUCCESS);
	};

	let runtime = tokio::runtime::Runtime::new()?;
	// The QUIC client binds its socket on the current runtime.
	let _runtime = runtime.enter();
	let relays = config.relays()?;
	let gpu = pollster::block_on(engine::Gpu::new(wgpu::Backends::all()))?;
	let room = config.guests.load(config.client.url.as_ref())?.map(Arc::new);
	if let Some(room) = &room {
		let renewal = guests::Renewal::connect(room.clone(), &config.client, config.quic.clone())?;
		runtime.spawn(renewal.run());
	}
	let launch = engine::Launch {
		gpu: gpu.clone(),
		relays,
		broadcast: config.broadcast.clone(),
		on_demand: config.on_demand,
		recordings,
		room,
	};
	let exists = scenes
		.try_exists()
		.with_context(|| format!("reading {}", scenes.display()))?;
	if !exists && let Some(dir) = scenes.parent().filter(|dir| !dir.as_os_str().is_empty()) {
		// Setup creates the file, but not the data directory a first launch
		// from the desktop names.
		std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
	}
	let app = match exists {
		true => {
			let (autosave, loaded) = autosave::Autosave::open(scenes.clone())?;
			App::Starting(runtime.block_on(engine::Engine::start(launch, autosave, loaded))?)
		}
		// Nothing is saved until setup starts the studio, so quitting before then
		// leaves no file and the next launch runs setup again.
		false => App::Setup(Box::new(setup::Setup::new(
			launch,
			scenes.clone(),
			runtime.handle().clone(),
		)?)),
	};

	run(app, gpu, runtime.handle().clone())?;
	Ok(ExitCode::SUCCESS)
}

/// End the studio on the way out: finish a recording, save the edits since
/// the last autosave, and close each relay session.
fn shutdown(engine: &engine::Engine, runtime: &tokio::runtime::Handle) -> anyhow::Result<()> {
	// A recording still running reads the program until stopping ends its
	// tracks, so it holds what the encoders publish on the way out, and then
	// finishes as a regular MP4.
	engine.recorder.close();
	let saved = engine.save();
	engine.stop();
	// Before the runtime goes, which would drop each relay session with the
	// tail of the broadcast still on its way.
	runtime.block_on(async { tokio::join!(engine.close(), engine.recorder.done()) });
	saved
}

/// What the window shows.
enum App {
	/// First launch: guided setup, until it starts the studio.
	Setup(Box<setup::Setup>),
	/// An engine whose studio is built once the window can draw it.
	Starting(Arc<engine::Engine>),
	Studio(Box<ui::Studio>),
}

/// The window's contents, and what building the studio takes.
struct Window {
	app: App,
	render: eframe::egui_wgpu::RenderState,
	runtime: tokio::runtime::Handle,
	/// The engine once the studio is built, for ending it on quit.
	started: Arc<OnceLock<Arc<engine::Engine>>>,
	keyboard: hotkeys::Keyboard,
	/// The window's own effects, on the setup screen and in the studio.
	fx: fx::Fx,
	/// Whether the studio's title bar is the window's frame, rather than the
	/// desktop's, which guided setup keeps.
	framed: bool,
	/// Set once the window is asked to close, as [`Events::closed`] is.
	closed: Arc<AtomicBool>,
}

impl eframe::App for Window {
	fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
		// The title bar's Close asks egui rather than the desktop, so no
		// window event says so.
		if ctx.input(|input| input.viewport().close_requested()) {
			self.closed.store(true, Ordering::Relaxed);
		}
		if let App::Studio(studio) = &mut self.app {
			studio.logic(ctx, frame);
		}
	}

	fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
		self.fx.begin(ui.ctx());
		self.show(ui, frame);
		self.fx.finish();
	}

	fn save(&mut self, storage: &mut dyn eframe::Storage) {
		self.fx.save(storage);
		if let App::Studio(studio) = &mut self.app {
			studio.save(storage);
		}
	}
}

impl Window {
	/// Draw setup or the studio, building the studio once its engine starts.
	fn show(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
		let engine = match &mut self.app {
			App::Studio(studio) => return studio.ui(ui, frame, &mut self.fx),
			App::Setup(setup) => match setup.ui(ui, &mut self.fx) {
				Some(engine) => engine,
				None => return,
			},
			App::Starting(engine) => engine.clone(),
		};
		let _ = self.started.set(engine.clone());
		// After setup, the desktop's frame goes. macOS cannot draw under its
		// title bar once the window is open, so it keeps it until a restart.
		if !self.framed && !cfg!(target_os = "macos") {
			ui.ctx().send_viewport_cmd(egui::ViewportCommand::Decorations(false));
			self.framed = true;
		}
		let mut studio = ui::Studio::new(&self.render, engine, self.runtime.clone(), self.framed);
		studio.load(hotkeys::Window {
			frame,
			ctx: ui.ctx(),
			keyboard: self.keyboard.clone(),
		});
		self.app = App::Studio(Box::new(studio));
		ui.ctx().request_repaint();
	}
}

/// eframe's handler for the window's events, which hears keys before egui,
/// so a hotkey is the key where it sits.
struct Events<'a> {
	eframe: eframe::EframeWinitApplication<'a>,
	keyboard: hotkeys::Keyboard,
	/// Whether the window was asked to close, by the desktop or the studio's
	/// own title bar, since eframe only logs the error that stops it otherwise.
	closed: Arc<AtomicBool>,
	/// The engine once the studio is built, to end on the way out.
	started: Arc<OnceLock<Arc<engine::Engine>>>,
	runtime: tokio::runtime::Handle,
	/// Whether ending the studio could not save it.
	unsaved: bool,
}

impl winit::application::ApplicationHandler<eframe::UserEvent> for Events<'_> {
	fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: winit::event::StartCause) {
		self.eframe.new_events(event_loop, cause);
	}

	fn resumed(&mut self, event_loop: &ActiveEventLoop) {
		self.eframe.resumed(event_loop);
	}

	fn user_event(&mut self, event_loop: &ActiveEventLoop, event: eframe::UserEvent) {
		self.eframe.user_event(event_loop, event);
	}

	fn window_event(&mut self, event_loop: &ActiveEventLoop, id: winit::window::WindowId, event: WindowEvent) {
		match &event {
			// A synthetic press is a key held as the window gained focus.
			WindowEvent::KeyboardInput {
				event,
				is_synthetic: false,
				..
			} if event.state.is_pressed() => {
				let name = event.key_without_modifiers();
				if !self.keyboard.pressed(event.physical_key, name, event.repeat) {
					return;
				}
			}
			WindowEvent::ModifiersChanged(modifiers) => self.keyboard.modifiers(modifiers.state()),
			WindowEvent::CloseRequested => self.closed.store(true, Ordering::Relaxed),
			_ => {}
		}
		self.eframe.window_event(event_loop, id, event);
	}

	fn device_event(
		&mut self,
		event_loop: &ActiveEventLoop,
		id: winit::event::DeviceId,
		event: winit::event::DeviceEvent,
	) {
		self.eframe.device_event(event_loop, id, event);
	}

	fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
		self.eframe.about_to_wait(event_loop);
	}

	fn suspended(&mut self, event_loop: &ActiveEventLoop) {
		self.eframe.suspended(event_loop);
	}

	fn exiting(&mut self, event_loop: &ActiveEventLoop) {
		self.eframe.exiting(event_loop);
		// Here rather than once the loop returns: quitting from the macOS app
		// menu or Dock, or logging out, ends the process as this returns.
		if let Some(engine) = self.started.get()
			&& let Err(err) = shutdown(engine, &self.runtime)
		{
			tracing::error!(error = format!("{err:#}"), "cannot save on quit");
			self.unsaved = true;
		}
	}

	fn memory_warning(&mut self, event_loop: &ActiveEventLoop) {
		self.eframe.memory_warning(event_loop);
	}
}

/// The app icon, which the build rasterizes from its SVG, for the systems
/// that take it from the window rather than the desktop entry or bundle.
fn icon() -> egui::IconData {
	let size = env!("ICON_SIZE").parse().expect("the build sets a numeric icon size");
	egui::IconData {
		rgba: include_bytes!(concat!(env!("OUT_DIR"), "/icon.rgba")).to_vec(),
		width: size,
		height: size,
	}
}

/// Show the window until it is closed, then end the studio.
fn run(app: App, gpu: engine::Gpu, runtime: tokio::runtime::Handle) -> anyhow::Result<()> {
	let started = Arc::new(OnceLock::new());
	// The studio's title bar is the window's: the desktop draws no frame, and
	// macOS draws its buttons over the studio's bar. Guided setup keeps the
	// desktop's until it starts the studio.
	let framed = matches!(app, App::Starting(_));
	let mut viewport = egui::ViewportBuilder::default()
		.with_title("kixel.live Studio")
		// The Wayland app id and X11 class, which the desktop finds the
		// app's entry and icon by: the entry's file name, without `.desktop`.
		.with_app_id("live.kixel.studio")
		.with_icon(icon())
		.with_inner_size([1440.0, 900.0])
		.with_min_inner_size(ui::MIN_SIZE);
	if framed {
		viewport = match cfg!(target_os = "macos") {
			true => viewport
				.with_fullsize_content_view(true)
				.with_titlebar_shown(false)
				.with_title_shown(false),
			false => viewport.with_decorations(false),
		};
	}
	let options = eframe::NativeOptions {
		viewport,
		wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
			// The window draws on the device the compositor does, so preview and
			// program reach the screen without leaving the GPU.
			wgpu_setup: eframe::egui_wgpu::WgpuSetup::Existing(eframe::egui_wgpu::WgpuSetupExisting {
				instance: gpu.instance,
				adapter: gpu.adapter,
				device: gpu.device,
				queue: gpu.queue,
			}),
			..Default::default()
		},
		// eframe would otherwise name the settings directory after the app id;
		// keep it beside the browser profile, under the app's name.
		persistence_path: data_dir().map(|dir| dir.join("app.ron")),
		..Default::default()
	};
	let event_loop = EventLoop::<eframe::UserEvent>::with_user_event()
		.build()
		.context("cannot open the window")?;
	let keyboard = hotkeys::Keyboard::default();
	let window_keyboard = keyboard.clone();
	let window_started = started.clone();
	let window_runtime = runtime.clone();
	let closed = Arc::new(AtomicBool::new(false));
	let window_closed = closed.clone();
	let eframe = eframe::create_native(
		"moq-studio",
		options,
		Box::new(move |cc| {
			let render = cc
				.wgpu_render_state
				.clone()
				.context("the window is not drawn with wgpu")?;
			kit::install(&cc.egui_ctx);
			let fx = fx::Fx::new(&render, cc.storage, &cc.egui_ctx, &window_runtime);
			Ok(Box::new(Window {
				app,
				render,
				runtime: window_runtime,
				started: window_started,
				keyboard: window_keyboard,
				fx,
				framed,
				closed: window_closed,
			}))
		}),
		&event_loop,
	);
	let mut events = Events {
		eframe,
		keyboard,
		closed: closed.clone(),
		started,
		runtime,
		unsaved: false,
	};
	event_loop.run_app(&mut events).context("the window failed")?;
	anyhow::ensure!(
		closed.load(Ordering::Relaxed),
		"the window failed; the error is logged above"
	);
	anyhow::ensure!(!events.unsaved, "cannot save on quit; the error is logged above");
	Ok(())
}

#[cfg(test)]
mod tests {
	use std::path::Path;
	use std::time::Duration;

	use super::*;

	#[test]
	fn parses_a_scene_file_and_a_relay() {
		let config = Config::parse_from(&[
			std::ffi::OsStr::new("show.toml"),
			std::ffi::OsStr::new("--connect"),
			std::ffi::OsStr::new("http://localhost:4443"),
		])
		.expect("a scene file and a relay should parse");
		config.check_deprecated().unwrap();
		let files = config.files().unwrap().unwrap();
		assert_eq!(files.scenes, Path::new("show.toml"));
		assert_eq!(files.recordings, Path::new("."), "recordings go beside the scene file");
		assert_eq!(config.broadcast, "studio");
		assert!(!config.on_demand, "the program encodes whatever the demand by default");
		assert!(config.guests.room.is_none(), "guests are opt-in");
		assert_eq!(
			Duration::from(config.guests.guest_expiry),
			Duration::from_secs(4 * 3600)
		);
		assert_eq!(config.client.url.unwrap().port(), Some(4443));
	}

	/// A failover relay is accepted only on one protocol version that carries
	/// broadcast instances, so a viewer moving between the relays resumes rather
	/// than restarts.
	#[test]
	fn failover_needs_one_version_with_instances() {
		let check = |extra: &[&str]| {
			let mut args = vec!["show.toml", "--connect", "https://a/", "--failover", "https://b/"];
			args.extend(extra);
			let args: Vec<_> = args.into_iter().map(std::ffi::OsStr::new).collect();
			Config::parse_from(&args).unwrap().check_failover()
		};
		let lite07 = ["--connect-version", "moq-lite-07-wip"];
		check(&lite07).expect("one version with instances");
		check(&[&lite07[..], &["--connect-bind", "127.0.0.1:0"]].concat()).expect("an ephemeral port");
		for refused in [
			&[][..],
			&["--connect-version", "moq-lite-06"],
			&["--connect-version", "moq-transport-22"],
			&[&lite07[..], &["--connect-version", "moq-lite-06"]].concat(),
			// Each relay binds its own socket.
			&[&lite07[..], &["--connect-bind", "127.0.0.1:4444"]].concat(),
		] {
			assert!(check(refused).is_err(), "{refused:?} was accepted");
		}
	}

	/// `--watch` opens no scene file and names its own relay.
	#[test]
	fn watch_takes_no_scene_file_or_relay() {
		let parse = |args: &[&str]| {
			let args: Vec<_> = args.iter().map(std::ffi::OsStr::new).collect();
			Config::parse_from(&args).unwrap()
		};
		let watch = parse(&["--watch", "https://relay.example/anon/studio"]);
		assert_eq!(watch.files().unwrap(), None);
		for refused in [
			&["show.toml", "--watch", "https://a/b"][..],
			&["--watch", "https://a/b", "--connect", "https://a/"],
			&["--watch", "https://a/b", "--room", "guests"],
		] {
			assert!(parse(refused).files().is_err(), "{refused:?} was accepted");
		}
	}

	/// With no scene file named, as from the desktop, the studio opens the one
	/// in its data directory and records to the videos folder, unless told
	/// where.
	#[test]
	fn defaults_to_the_collection_in_the_data_directory() {
		let files = Files::new(None, None).unwrap();
		assert!(files.scenes.is_absolute());
		assert!(files.recordings.is_absolute());
		assert_ne!(Some(files.recordings.as_path()), files.scenes.parent());

		let files = Files::new(None, Some("clips".into())).unwrap();
		assert_eq!(files.recordings, Path::new("clips"));
		let files = Files::new(Some("shows/main.toml".into()), None).unwrap();
		assert_eq!(files.recordings, Path::new("shows"));
	}
}
