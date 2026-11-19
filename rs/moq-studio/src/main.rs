//! kixel.live Studio: compose scenes from cameras, screens, images, and other
//! broadcasts on the GPU, and go live over MoQ.
//!
//! The scenes, sources, and compositor live in `moq-scene`; this binary is the
//! window around them and the program clock that publishes the result.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
mod library;
mod markers;
mod mix;
mod past;
mod pointer;
mod record;
mod replay;
mod setup;
mod shelf;
#[cfg(test)]
mod shots;
mod show;
mod snap;
mod stream;
mod ui;
mod uplink;
mod vcam;
mod watch;

#[derive(usage_rs::Cli, Clone)]
#[usage(unknown_flags = "error", args_override_self = false)]
#[usage(name = "moq-studio")]
#[usage(completion, settings, unknown_env(prefix = "MOQ_"))]
pub struct Config {
	/// The show to open: its scene file, saved as it changes. If it does not
	/// exist, guided setup proposes one and creates it. Without one, the
	/// window opens on your recent shows.
	#[usage(value_hint = usage_rs::ValueHint::FilePath, extensions("toml"))]
	pub scenes: Option<PathBuf>,

	/// Play a studio broadcast instead, as a viewer sees it: the program with
	/// each source published independently over it. The URL's path names the
	/// broadcast, as `moq play`'s does, such as
	/// `https://relay.example/anon/studio`. A prototype, until `moq play` plays
	/// the layer layout.
	#[usage(long)]
	pub watch: Option<url::Url>,

	/// The broadcast path the program goes live at, for this run, over the
	/// show's own (Settings, Stream), which is `studio` until set.
	#[usage(long)]
	pub broadcast: Option<String>,

	/// Copy and encode the program video at the canvas size only while someone
	/// watches it, as the half size always is, for this run. This saves the
	/// work while nobody does, but leaves a late viewer nothing from before
	/// they arrived. Audio is always encoded, and a recording has an encoder
	/// of its own.
	#[usage(long)]
	pub on_demand: bool,

	/// The directory recordings and clips are saved in, for this run.
	/// Defaults to the scene file's.
	#[usage(long, value_hint = usage_rs::ValueHint::DirPath)]
	pub recordings: Option<PathBuf>,

	/// Guests who join from a web page, appearing as sources.
	#[usage(flatten)]
	pub guests: guests::Config,

	/// The MoQ client configuration. `--connect` names the relay to go live
	/// on for this run, over the show's own, and `--connect-token` goes to it
	/// alone. Nothing is published until you go live.
	#[usage(flatten)]
	pub client: moq_tokio::connect::Config,

	/// A second relay to publish the same broadcast to, for this run, so a
	/// viewer linked to both resumes from it when the first fails. Both must
	/// speak the one `--connect-version`, which must carry broadcast instances.
	/// Its credential is its own, from Settings or its URL's `?jwt=`.
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
	/// Refuse `--watch` beside the studio's own flags.
	fn check_watch(&self) -> anyhow::Result<()> {
		if self.watch.is_none() {
			return Ok(());
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
		Ok(())
	}

	/// Refuse a config parsed from released spellings, naming what replaced each.
	fn check_deprecated(&self) -> anyhow::Result<()> {
		let mut deprecated = self.client.deprecated();
		deprecated.extend(self.quic.deprecated());
		anyhow::ensure!(deprecated.is_empty(), "{deprecated}");
		Ok(())
	}

	/// What the flags set for every show this run opens.
	fn flags(&self) -> stream::Flags {
		stream::Flags {
			relay: self.client.url.clone(),
			tokens: self.client.tokens.clone(),
			failover: self.failover.clone(),
			broadcast: self.broadcast.clone(),
			on_demand: self.on_demand,
			recordings: self.recordings.clone(),
			guests: self.guests.clone(),
		}
	}

	/// Every other client flag, which each relay is dialed with: the relay and
	/// its tokens come from the show, or the flags over it.
	fn client(&self) -> moq_tokio::connect::Config {
		let mut client = self.client.clone();
		client.url = None;
		client.tokens.clear();
		client
	}
}

/// Where the studio keeps its own files, such as the window's settings.
fn data_dir() -> Option<PathBuf> {
	eframe::storage_dir("moq-studio")
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
	config.check_watch()?;
	if let Some(url) = config.watch.clone() {
		watch::run(config.client.clone(), config.quic.clone(), url)?;
		return Ok(ExitCode::SUCCESS);
	}

	let runtime = tokio::runtime::Runtime::new()?;
	// The QUIC client binds its socket on the current runtime.
	let _runtime = runtime.enter();
	let gpu = pollster::block_on(engine::Gpu::new(wgpu::Backends::all()))?;
	let opener = show::Opener {
		gpu: gpu.clone(),
		runtime: runtime.handle().clone(),
		flags: config.flags(),
		client: config.client(),
		quic: config.quic.clone(),
		credentials: stream::Credentials::system(),
	};
	let app = match &config.scenes {
		// Nothing is saved until setup starts the studio, so quitting before
		// then leaves no file and the next launch runs setup again.
		Some(scenes) => match runtime.block_on(opener.open(scenes.clone()))? {
			show::Opened::Studio(engine, stream) => App::Starting(engine, stream),
			show::Opened::Setup(setup, stream) => App::Setup(setup, stream),
		},
		None => App::Library(Box::new(library::Library::new(library::pictures())), None),
	};

	run(app, opener)?;
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

/// The engine of the show open, to end on quit or when it is closed.
#[derive(Default)]
struct Started(Mutex<Option<Arc<engine::Engine>>>);

impl Started {
	fn get(&self) -> Option<Arc<engine::Engine>> {
		self.0.lock().unwrap().clone()
	}

	fn set(&self, engine: Arc<engine::Engine>) {
		*self.0.lock().unwrap() = Some(engine);
	}

	/// Take the engine, so a quit after it is closed ends nothing twice.
	fn take(&self) -> Option<Arc<engine::Engine>> {
		self.0.lock().unwrap().take()
	}
}

/// A show being opened from the library.
type Opening = setup::Task<anyhow::Result<show::Opened>>;

/// What the window shows.
enum App {
	/// No show open: the library, and the show it is opening.
	Library(Box<library::Library>, Option<Opening>),
	/// A new show: guided setup, until it starts the studio.
	Setup(Box<setup::Setup>, stream::Stream),
	/// An engine whose studio is built once the window can draw it.
	Starting(Arc<engine::Engine>, stream::Stream),
	Studio(Box<ui::Studio>),
	/// Between two of the above, within the frame that moves from one to the
	/// next.
	Moving,
}

/// The window's contents, and what building the studio takes.
struct Window {
	app: App,
	render: eframe::egui_wgpu::RenderState,
	opener: show::Opener,
	/// The engine once the studio is built, until the show is closed.
	started: Arc<Started>,
	/// The shows this user has opened.
	recent: library::Recent,
	/// Where the shows' pictures are kept, if the system has app storage.
	pictures: Option<PathBuf>,
	/// The open show's last stream off air that [`Self::recent`] has noted.
	aired: Option<engine::Aired>,
	keyboard: hotkeys::Keyboard,
	/// The window's own effects, on the setup screen and in the studio.
	fx: fx::Fx,
	/// Whether the studio's title bar is the window's frame, rather than the
	/// desktop's, which the library and guided setup keep.
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
		self.follow_air();
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
		self.follow_air();
		if let Some(engine) = self.started.get() {
			let path = engine.autosave.path();
			// Its time on air so far, in case the window closes while live.
			if let Some(live) = engine.on_air().live() {
				let lasted = live.since.elapsed().as_secs();
				let at = library::now().saturating_sub(lasted);
				self.recent.aired(path, library::Aired { at, lasted });
			}
			if self.closed.load(Ordering::Relaxed) {
				self.keep_picture(&engine);
			}
		}
		self.recent.save(storage);
	}
}

impl Window {
	/// Draw the library, setup, or the studio, building the studio once its
	/// engine starts.
	fn show(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
		match std::mem::replace(&mut self.app, App::Moving) {
			App::Studio(mut studio) => {
				studio.ui(ui, frame, &mut self.fx);
				match studio.closing() {
					true => self.close_show(studio, ui.ctx(), frame),
					false => self.app = App::Studio(studio),
				}
			}
			App::Library(library, opening) => self.library(ui, library, opening),
			App::Setup(mut setup, stream) => match setup.ui(ui, &mut self.fx) {
				Some(engine) => {
					self.app = App::Starting(engine, stream);
					ui.ctx().request_repaint();
				}
				None => self.app = App::Setup(setup, stream),
			},
			App::Starting(engine, stream) => self.start(ui, frame, engine, stream),
			App::Moving => unreachable!("the window moves between pages within a frame"),
		}
	}

	/// Draw the library, and open the show picked there, off this thread.
	fn library(&mut self, ui: &mut egui::Ui, mut library: Box<library::Library>, mut opening: Option<Opening>) {
		if let Some(task) = &mut opening {
			ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
			if task.poll().is_some() {
				let Some(setup::Task::Done(opened)) = opening.take() else {
					unreachable!("polled done")
				};
				library.opening = None;
				match opened {
					Ok(show::Opened::Studio(engine, stream)) => {
						self.app = App::Starting(engine, stream);
						return ui.ctx().request_repaint();
					}
					Ok(show::Opened::Setup(setup, stream)) => {
						self.app = App::Setup(setup, stream);
						return ui.ctx().request_repaint();
					}
					Err(err) => library.error = Some(format!("{err:#}")),
				}
			}
		}
		let runtime = self.opener.runtime.clone();
		if let Some(library::Pick::Open(path)) = library.ui(ui, &mut self.recent, &runtime) {
			let name = path.file_stem().unwrap_or(path.as_os_str()).to_string_lossy();
			library.opening = Some(name.into_owned());
			library.error = None;
			let opener = self.opener.clone();
			match setup::Task::spawn(&runtime, move || async move { opener.open(path).await }) {
				Ok(task) => opening = Some(task),
				Err(err) => {
					library.opening = None;
					library.error = Some(format!("cannot open the show: {err}"));
				}
			}
		}
		self.app = App::Library(library, opening);
	}

	/// Build the studio around `engine`, now that the window can draw it.
	fn start(
		&mut self,
		ui: &mut egui::Ui,
		frame: &mut eframe::Frame,
		engine: Arc<engine::Engine>,
		stream: stream::Stream,
	) {
		self.started.set(engine.clone());
		self.recent.opened(engine.autosave.path(), library::now());
		// Once a show is open, the desktop's frame goes. macOS cannot draw under
		// its title bar once the window is open, so it keeps it until a restart.
		if !self.framed && !cfg!(target_os = "macos") {
			ui.ctx().send_viewport_cmd(egui::ViewportCommand::Decorations(false));
			self.framed = true;
		}
		let runtime = self.opener.runtime.clone();
		let mut studio = ui::Studio::new(&self.render, engine, stream, runtime, self.framed);
		studio.load(hotkeys::Window {
			frame,
			ctx: ui.ctx(),
			keyboard: self.keyboard.clone(),
		});
		self.app = App::Studio(Box::new(studio));
		ui.ctx().request_repaint();
	}

	/// Keep the program as the open show's picture in the library.
	fn keep_picture(&mut self, engine: &engine::Engine) {
		let path = engine.autosave.path();
		let saved = self
			.pictures
			.clone()
			.context("this system has no app storage")
			.and_then(|pictures| {
				let picture = library::capture(&self.opener.gpu, engine)?;
				self.recent.shaped(path, picture.portrait());
				library::save_picture(&pictures, path, &picture)
			});
		if let Err(err) = saved {
			tracing::warn!(error = format!("{err:#}"), "cannot keep the show's picture");
		}
	}

	/// End the open show as quitting does, and return to the library with its
	/// card refreshed. The show stays open if it cannot be saved, or went live
	/// since the click, rather than lose an edit or cut a stream.
	fn close_show(&mut self, mut studio: Box<ui::Studio>, ctx: &egui::Context, frame: &mut eframe::Frame) {
		let engine = studio.engine().clone();
		// Claimed against a go-live first, which the engine being off air
		// alone would not refuse while it reads tokens or checks encoders.
		let ready = studio
			.reserve_close()
			.and_then(|()| engine.save().inspect_err(|_| studio.release_close()));
		if let Err(err) = ready {
			studio.refuse(err.context("cannot close the show"));
			self.app = App::Studio(studio);
			return;
		}
		self.follow_air();
		// Before the engine stops drawing.
		self.keep_picture(&engine);
		if let Some(storage) = frame.storage_mut() {
			studio.save(storage);
			self.recent.save(storage);
		}
		self.started.take();
		self.aired = None;
		if let Err(err) = shutdown(&engine, &self.opener.runtime) {
			tracing::error!(error = format!("{err:#}"), "cannot save on close");
		}
		// The studio goes with its hotkeys, which would otherwise fire on the
		// closed show.
		studio.release(&self.render);
		// The desktop's frame again, which the library keeps.
		if self.framed && !cfg!(target_os = "macos") {
			ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
			self.framed = false;
		}
		self.app = App::Library(Box::new(library::Library::new(self.pictures.clone())), None);
		ctx.request_repaint();
	}

	/// Note when the open show's last stream went live, and how long it
	/// stayed, once it ends.
	fn follow_air(&mut self) {
		let Some(engine) = self.started.get() else { return };
		let Some(aired) = engine.aired() else { return };
		if self.aired == Some(aired) {
			return;
		}
		self.aired = Some(aired);
		let at = library::now().saturating_sub(aired.since.elapsed().as_secs());
		let lasted = aired.lasted.as_secs();
		self.recent.aired(engine.autosave.path(), library::Aired { at, lasted });
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
	/// The studio's own window, the first to hear an event: the multiview's
	/// opens after it, closing that one closes nothing else, and typing into
	/// the studio's holds back no key pressed in it.
	root: Option<winit::window::WindowId>,
	/// The engine once the studio is built, to end on the way out.
	started: Arc<Started>,
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
		let root = *self.root.get_or_insert(id) == id;
		match &event {
			// A synthetic press is a key held as the window gained focus.
			WindowEvent::KeyboardInput {
				event,
				is_synthetic: false,
				..
			} if event.state.is_pressed() => {
				let name = event.key_without_modifiers();
				if !self.keyboard.pressed(event.physical_key, name, event.repeat, root) {
					return;
				}
			}
			WindowEvent::ModifiersChanged(modifiers) => self.keyboard.modifiers(modifiers.state()),
			WindowEvent::CloseRequested if root => self.closed.store(true, Ordering::Relaxed),
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
			&& let Err(err) = shutdown(&engine, &self.runtime)
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
fn run(app: App, opener: show::Opener) -> anyhow::Result<()> {
	let started = Arc::<Started>::default();
	// The studio's title bar is the window's: the desktop draws no frame, and
	// macOS draws its buttons over the studio's bar. The library and guided
	// setup keep the desktop's until a show opens.
	let framed = matches!(app, App::Starting(..));
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
	let gpu = opener.gpu.clone();
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
	let runtime = opener.runtime.clone();
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
			let fx = fx::Fx::new(&render, cc.storage, &cc.egui_ctx, &opener.runtime);
			let recent = library::Recent::load(cc.storage);
			Ok(Box::new(Window {
				app,
				render,
				opener,
				started: window_started,
				recent,
				pictures: library::pictures(),
				aired: None,
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
		root: None,
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
	use super::*;

	fn parse(args: &[&str]) -> Config {
		let args: Vec<_> = args.iter().map(std::ffi::OsStr::new).collect();
		Config::parse_from(&args).unwrap()
	}

	/// A scene file and a relay parse as flags over the show's own settings,
	/// with the relay and its tokens taken out of the client every relay is
	/// dialed with.
	#[test]
	fn parses_a_scene_file_and_a_relay() {
		let config = parse(&[
			"show.toml",
			"--connect",
			"http://localhost:4443",
			"--connect-token",
			"secret",
		]);
		config.check_deprecated().unwrap();
		config.check_watch().unwrap();
		assert_eq!(config.scenes.as_deref(), Some(std::path::Path::new("show.toml")));
		let flags = config.flags();
		assert_eq!(flags.broadcast, None, "the show's broadcast name stands");
		assert!(!flags.on_demand, "the program encodes whatever the demand by default");
		assert!(flags.guests.room.is_none(), "guests are opt-in");
		assert_eq!(flags.relay.unwrap().port(), Some(4443));
		assert_eq!(flags.tokens.len(), 1);
		let client = config.client();
		assert!(client.url.is_none() && client.tokens.is_empty());
	}

	/// With no arguments the studio opens on the library.
	#[test]
	fn opens_with_no_arguments() {
		let config = parse(&[]);
		config.check_watch().unwrap();
		assert!(config.scenes.is_none() && config.watch.is_none());
	}

	/// `--watch` opens no scene file and names its own relay.
	#[test]
	fn watch_takes_no_scene_file_or_relay() {
		parse(&["--watch", "https://relay.example/anon/studio"])
			.check_watch()
			.unwrap();
		for refused in [
			&["show.toml", "--watch", "https://a/b"][..],
			&["--watch", "https://a/b", "--connect", "https://a/"],
			&["--watch", "https://a/b", "--room", "guests"],
		] {
			assert!(parse(refused).check_watch().is_err(), "{refused:?} was accepted");
		}
	}
}
