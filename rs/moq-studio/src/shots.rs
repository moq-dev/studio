//! Named views of the window, rendered to PNG with no display: `just shots`.
//!
//! Each view is a test that opens the show in `shots/show.toml` on a fresh
//! engine and draws one state of the window through egui_kittest, on the GPU
//! the compositor uses. A still view draws the same picture every time, which
//! its test checks by drawing it twice; a live one shows something that moves,
//! such as an encoder's timings. One more test drives the window the same way,
//! clicking as a person does, and draws nothing.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use eframe::egui_wgpu;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use moq_scene::source::Status;

use crate::fx::Effects;
use crate::ui::{Page, Setting, Tool};
use crate::{App, Window, autosave, engine, fx, hotkeys, library, setup, show, stream};

/// The window's size, the mock-ups'.
const SIZE: egui::Vec2 = egui::vec2(1440.0, 900.0);

/// A window snapped to half of a 1440x900 screen, as the compact mock-up's.
const HALF: egui::Vec2 = egui::vec2(711.0, 872.0);

/// The longest a view waits for its sources and pictures to settle.
const SETTLE: Duration = Duration::from_secs(20);

/// What a view opens.
enum Open {
	/// Guided setup, as the first launch shows it.
	Setup,
	/// The library, as the app opens with no show, with a few recent shows.
	Library,
	/// The studio on the show in `shots/<show>`, in a window `size` large,
	/// turned to `view`, with a relay of its own to go live on if `relay`.
	Studio {
		view: View,
		size: egui::Vec2,
		show: &'static str,
		relay: bool,
	},
	/// The kit's gallery, with a sheet open over it or not.
	Kit { sheet: bool },
}

/// Where a view turns the studio.
enum View {
	Page(Page),
	/// The BUILD page, showing a tool.
	Tool(Tool),
	/// Settings, at a tab.
	Setting(Setting),
	/// The stream's health in detail.
	Health,
	/// The LIVE page after a mute, with the health dot's popover open and the
	/// pointer on undo, which names the mute.
	Hover,
	/// The go-live sheet, once its checks finish.
	Going,
	/// On air, at the LIVE page.
	OnAir,
	/// On air, with the pointer on the clock.
	EndStream,
	/// On air, with the health dot's popover open.
	OnAirHealth,
}

/// The studio on the shots' show, at the mock-ups' size, turned to `view`.
fn studio(view: View) -> Open {
	Open::Studio {
		view,
		size: SIZE,
		show: "show.toml",
		relay: false,
	}
}

/// [`studio`], with a relay to go live on.
fn relayed(view: View) -> Open {
	Open::Studio {
		view,
		size: SIZE,
		show: "show.toml",
		relay: true,
	}
}

/// Whether a view draws the same picture every time.
#[derive(PartialEq)]
enum Motion {
	Still,
	Live,
}

/// A test per view, named for `just shots <name>`.
macro_rules! views {
	($($(#[doc = $doc:literal])* $name:ident: $motion:ident, $open:expr;)*) => {$(
		$(#[doc = $doc])*
		#[test]
		#[ignore = "draws on a GPU; run with `just shots`"]
		fn $name() {
			shot(stringify!($name), Motion::$motion, $open);
		}
	)*};
}

// Every view. A quest that adds a page or a state adds its row, and an
// `Open` variant for a state no existing one reaches.
views! {
	/// Every widget of the kit in every state.
	kit: Still, Open::Kit { sheet: false };
	/// The kit's sheet, over the gallery.
	kit_sheet: Still, Open::Kit { sheet: true };
	/// Once its checks are done: this machine's encoder timings, and the
	/// capture errors of a build without `capture`, which keeps the view off
	/// the host's devices.
	setup: Live, Open::Setup;
	/// The LIVE page: the picture as large as the window allows, the pads, the
	/// panic key, and the sound.
	live: Still, studio(View::Page(Page::Live));
	/// What the title bar's controls show under the pointer.
	live_hover: Still, studio(View::Hover);
	/// A portrait show: the pages in a sidebar, and the scenes and the sound
	/// in columns beside the picture.
	live_portrait: Still, Open::Studio { view: View::Page(Page::Live), size: SIZE, show: "portrait.toml", relay: false };
	/// A window snapped beside a game stacks the picture, the scenes, and the
	/// sound.
	live_compact: Still, Open::Studio { view: View::Page(Page::Live), size: HALF, show: "show.toml", relay: false };
	build: Still, studio(View::Page(Page::Build));
	/// MIX holds the mixer, and a small live picture in the sound's place.
	mix: Still, studio(View::Page(Page::Mix));
	/// MIX in a stacked window, its live picture as tall as the sound's slot
	/// allows.
	mix_compact: Still, Open::Studio { view: View::Page(Page::Mix), size: HALF, show: "show.toml", relay: false };
	review: Still, studio(View::Page(Page::Review));
	graph: Still, studio(View::Setting(Setting::Graph));
	history: Still, studio(View::Setting(Setting::History));
	devices: Still, studio(View::Tool(Tool::Devices));
	guests: Still, studio(View::Tool(Tool::Guests));
	keyframes: Still, studio(View::Tool(Tool::Keyframes));
	health: Still, studio(View::Health);
	/// Settings' Show tab, with the encoders' timings.
	output: Live, studio(View::Setting(Setting::Show));
	hotkeys: Still, studio(View::Setting(Setting::Hotkeys));
	/// With no show open: recent shows, each with its picture, beside New
	/// show and Open a show file.
	library: Still, Open::Library;
	/// GO LIVE's sheet: the relay with its switch and its state, and the
	/// checks once they finish.
	going: Live, relayed(View::Going);
	/// On air: the accent frame, ON AIR, and the clock.
	on_air: Live, relayed(View::OnAir);
	/// On air, the pointer on the clock, which reads END STREAM.
	end_stream: Live, relayed(View::EndStream);
	/// On air, the health dot's popover: the worst problem, every
	/// destination, the viewers, and the encoder.
	on_air_health: Live, relayed(View::OnAirHealth);
	/// Settings' Stream tab: the relay, its token, the failover, the
	/// broadcast's name, and on demand.
	settings_stream: Still, studio(View::Setting(Setting::Stream));
}

/// Draw the view `name` opens into `$SHOTS_DIR/<name>.png`, twice if it is
/// still, failing if the two differ.
fn shot(name: &str, motion: Motion, open: Open) {
	let out = std::env::var_os("SHOTS_DIR").expect("SHOTS_DIR names where the PNGs go; run `just shots`");
	// Absolute, before the change of directory below.
	let out = std::path::absolute(out).unwrap();
	std::fs::create_dir_all(&out).unwrap();

	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	let image = shoot(&open, &gpu, &runtime, &fixture.join("shots"));
	let path = out.join(format!("{name}.png"));
	if motion == Motion::Still {
		let again = shoot(&open, &gpu, &runtime, &fixture.join("shots"));
		if image != again {
			// Both, to see what moved.
			let again_path = out.join(format!("{name}.again.png"));
			image::DynamicImage::ImageRgba8(again)
				.to_rgb8()
				.save(&again_path)
				.unwrap();
			image::DynamicImage::ImageRgba8(image).to_rgb8().save(&path).unwrap();
			panic!(
				"the still view `{name}` drew a different picture twice: {}",
				again_path.display()
			);
		}
	}
	// The window is opaque, and a quarter smaller without the alpha.
	image::DynamicImage::ImageRgba8(image).to_rgb8().save(&path).unwrap();
	println!("{}", path.display());
}

/// A directory of its own to open the show from, made the current one, a
/// runtime, and Mesa's software GPU.
///
/// The show is opened as `show.toml` from there, so the window names it the
/// same on every machine. nextest runs each test in a process of its own,
/// which the change of directory relies on.
fn lavapipe() -> (tempfile::TempDir, tokio::runtime::Runtime, engine::Gpu) {
	let show = tempfile::tempdir().unwrap();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	std::fs::copy(
		fixture.join("../moq-scene/src/test_data/person.jpg"),
		show.path().join("person.jpg"),
	)
	.unwrap();
	std::env::set_current_dir(show.path()).unwrap();

	let runtime = tokio::runtime::Runtime::new().unwrap();
	// Vulkan only, where `just shots` names lavapipe the sole driver: any other
	// backend would find the host's hardware and draw a different picture.
	let gpu = pollster::block_on(engine::Gpu::new(wgpu::Backends::VULKAN)).expect("a GPU");
	let info = gpu.adapter.get_info();
	assert_eq!(
		info.device_type,
		wgpu::DeviceType::Cpu,
		"views draw on lavapipe, not {}",
		info.name
	);
	(show, runtime, gpu)
}

/// How a view starts the studio: recording beside the show.
fn launch(gpu: &engine::Gpu) -> engine::Launch {
	engine::Launch {
		gpu: gpu.clone(),
		recordings: ".".into(),
		room: None,
	}
}

/// `show.toml`'s stream settings, on `relay` if given, with tokens in a store
/// of the test's own.
fn stream(relay: Option<&Relay>) -> stream::Stream {
	let mut flags = stream::Flags::default();
	let mut client = moq_tokio::connect::Config::default();
	if let Some(relay) = relay {
		flags.relay = Some(relay.url.clone());
		client.tls.fingerprint = vec![relay.fingerprint.clone()];
	}
	let credentials = stream::Credentials::memory();
	stream::Stream::load(Path::new("show.toml"), flags, client, Default::default(), credentials).unwrap()
}

/// A relay of the view's own, running on `runtime`.
struct Relay {
	url: url::Url,
	fingerprint: String,
}

impl Relay {
	fn start(runtime: &tokio::runtime::Runtime) -> Self {
		runtime.block_on(async {
			let _ = moq_tokio::crypto::install_default();
			let fixture = moq_relay::test_relay().await.expect("test relay");
			let ready = fixture.relay.ready();
			tokio::spawn(fixture.relay.run());
			ready.wait().await.expect("relay ready");
			Self {
				url: fixture.url,
				fingerprint: fixture.fingerprint,
			}
		})
	}
}

/// Start the studio by `launch` on `show.toml`, without the history an
/// earlier draw's edits left, streaming as `stream` sets.
fn start(launch: engine::Launch, stream: stream::Stream, runtime: &tokio::runtime::Runtime) -> App {
	let _ = std::fs::remove_file("show.history.toml");
	let (autosave, loaded) = autosave::Autosave::open("show.toml".into()).unwrap();
	App::Starting(
		runtime
			.block_on(engine::Engine::start(launch, autosave, loaded))
			.unwrap(),
		stream,
	)
}

/// A click on a pad takes its scene on air, and the panic key mutes every
/// microphone and guest, in an edit undo takes back, and cuts to the safe
/// scene: the window as a person uses it, on a real engine. Remote feeds that
/// are not guests stay on. Then the title bar's Close closes it. It draws no
/// view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn a_click_takes_and_the_panic_key_cuts() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	let mut show = std::fs::read_to_string(fixture.join("shots/show.toml")).unwrap();
	// Failed in a build without `capture`, which leaves it a microphone to mute.
	show.push_str("\n[sources.mic]\nkind = \"microphone\"\n");
	// A guest in the room the show's `guest` and `caller` feeds are under,
	// which are not guests' broadcasts.
	show.push_str("\n[sources.alice]\nkind = \"broadcast\"\npath = \"room/alice/camera.hang\"\n");
	std::fs::write("show.toml", show).unwrap();
	let connect = url::Url::parse("https://relay.example/show").unwrap();
	let page = url::Url::parse("https://studio.example/join/").unwrap();
	let key = crate::guests::tests::key("show/room", &["**"], &["**"]);
	let room = crate::guests::Room::new(&connect, "room", key, page, crate::guests::tests::EXPIRY).unwrap();
	let launch = engine::Launch {
		room: Some(Arc::new(room)),
		..launch(&gpu)
	};
	let app = start(launch, stream(None), &runtime);
	let (mut harness, started, closed) = window(app, &gpu, &runtime, SIZE, library::Recent::default());
	harness.step();
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	let program = || {
		engine
			.state
			.lock()
			.unwrap()
			.compositor
			.program_scene()
			.map(str::to_string)
	};
	let muted = |name: &str| {
		let state = engine.state.lock().unwrap();
		state.collection().audio.get(name).is_some_and(|audio| audio.muted)
	};
	assert_eq!(program().as_deref(), Some("Main"));

	harness.get_by_label("Interview").click();
	harness.run_steps(2);
	assert_eq!(program().as_deref(), Some("Interview"), "the pad took its scene");

	harness.get_by_label("Panic").click();
	harness.run_steps(2);
	assert_eq!(program().as_deref(), Some("Break"), "cut to the safe scene");
	assert!(muted("mic"), "the microphone is muted");
	assert!(muted("alice"), "the guest is muted");
	assert!(!muted("guest") && !muted("caller"), "other remote feeds stay on");

	harness
		.get_by_label("Undo: Panic: mute every microphone and guest")
		.click();
	harness.run_steps(2);
	assert!(!muted("mic") && !muted("alice"), "undo takes the mute back");
	assert_eq!(program().as_deref(), Some("Break"), "and leaves program where it is");

	// The title bar's Close asks egui to close, which eframe passes back as a
	// close event the window counts as asked for, not as a failure.
	harness.get_by_label("Close").click();
	harness.step();
	let commands = &harness.output().viewport_output[&egui::ViewportId::ROOT].commands;
	assert!(commands.contains(&egui::ViewportCommand::Close), "{commands:?}");
	assert!(!closed.load(Ordering::Relaxed));
	let root = harness.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default();
	root.events.push(egui::ViewportEvent::Close);
	harness.step();
	assert!(
		closed.load(Ordering::Relaxed),
		"the window counts the close as asked for"
	);

	drop(harness);
	quit(&engine, &runtime);
}

/// Draw what `open` opens on a fresh engine, from a scene file in `shows`.
fn shoot(open: &Open, gpu: &engine::Gpu, runtime: &tokio::runtime::Runtime, shows: &Path) -> image::RgbaImage {
	if let &Open::Kit { sheet } = open {
		return gallery(gpu, runtime, sheet);
	}
	if let Open::Library = open {
		let (app, recent) = shelf();
		let (mut harness, ..) = window(app, gpu, runtime, SIZE, recent);
		// Enough for the pictures to load and the cards to settle.
		harness.run_steps(4);
		return harness.render().unwrap();
	}
	let app = match open {
		Open::Setup => {
			let setup = setup::Setup::new(launch(gpu), None, "show.toml".into(), runtime.handle().clone());
			App::Setup(Box::new(setup.unwrap()), stream(None))
		}
		Open::Kit { .. } | Open::Library => unreachable!("drawn without an engine"),
		Open::Studio { show, relay, .. } => {
			std::fs::copy(shows.join(show), "show.toml").unwrap();
			let relay = relay.then(|| Relay::start(runtime));
			start(launch(gpu), stream(relay.as_ref()), runtime)
		}
	};
	let size = match open {
		Open::Studio { size, .. } => *size,
		_ => SIZE,
	};
	let (mut harness, started, _) = window(app, gpu, runtime, size, library::Recent::default());

	let Open::Studio { view, .. } = open else {
		runtime
			.block_on(kio::clock::timeout(SETTLE, checked(&mut harness)))
			.expect("setup finishes its checks");
		return harness.render().unwrap();
	};
	harness.step();
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	let controls = studio.controls();
	match view {
		View::Page(page) => studio.turn(*page),
		View::Tool(tool) => studio.tool(*tool),
		View::Setting(setting) => studio.open(*setting),
		View::Health => studio.details(),
		View::Going => studio.going(),
		View::Hover | View::OnAir | View::EndStream | View::OnAirHealth => {}
	}
	let engine = started.get().expect("the studio started").clone();
	if let View::OnAir | View::EndStream | View::OnAirHealth = view {
		controls.go_live().expect("going live starts");
		runtime
			.block_on(kio::clock::timeout(SETTLE, async {
				while engine.on_air().live().is_none() {
					kio::clock::sleep(Duration::from_millis(20)).await;
				}
			}))
			.unwrap_or_else(|_| panic!("the studio goes live: {:?}", controls.refused.lock().unwrap()));
	}
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	match view {
		View::Hover => hover(&mut harness),
		View::Going => runtime
			.block_on(kio::clock::timeout(SETTLE, checks(&mut harness)))
			.expect("the go-live checks finish"),
		View::EndStream => {
			harness.get_by_label("End stream").hover();
			harness.run_steps(2);
		}
		View::OnAirHealth => {
			let pill = ["Healthy", "Starting", "Degraded", "Unhealthy"]
				.into_iter()
				.find_map(|name| harness.query_by_label(name))
				.expect("the health dot");
			pill.click();
			harness.run_steps(2);
		}
		_ => {}
	}
	let image = harness.render().unwrap();
	drop(harness);
	quit(&engine, runtime);
	image
}

/// A window on `app`, `size` large, where the studio it starts is kept, and
/// whether it was asked to close.
fn window<'a>(
	app: App,
	gpu: &engine::Gpu,
	runtime: &'a tokio::runtime::Runtime,
	size: egui::Vec2,
	recent: library::Recent,
) -> (Harness<'a, Window>, Arc<OnceLock<Arc<engine::Engine>>>, Arc<AtomicBool>) {
	let opener = show::Opener {
		gpu: gpu.clone(),
		runtime: runtime.handle().clone(),
		flags: Default::default(),
		client: Default::default(),
		quic: Default::default(),
		credentials: stream::Credentials::memory(),
	};
	let render = render_state(gpu);
	let started = Arc::new(OnceLock::new());
	let kept = started.clone();
	let closed = Arc::new(AtomicBool::new(false));
	let window_closed = closed.clone();
	let harness = Harness::builder()
		.with_size(size)
		.renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(render.clone()))
		.build_eframe(move |cc| {
			crate::kit::install(&cc.egui_ctx);
			let mut fx = fx::Fx::new(&render, None, &cc.egui_ctx, runtime.handle());
			// Not the system's reduced motion, which differs between machines.
			fx.pick(Effects::Full);
			Window {
				app,
				render: render.clone(),
				opener,
				started: kept,
				recent,
				aired: None,
				keyboard: hotkeys::Keyboard::default(),
				fx,
				framed: true,
				closed: window_closed,
			}
		});
	(harness, started, closed)
}

/// Stop `engine` as quitting does.
fn quit(engine: &engine::Engine, runtime: &tokio::runtime::Runtime) {
	// Now, while the scene file is this test's: the autosave's thread outlives
	// the engine, and would otherwise write an edit made here over the next
	// draw's copy.
	engine.save().expect("the show saves");
	engine.recorder.close();
	engine.stop();
	runtime.block_on(async { tokio::join!(engine.close(), engine.recorder.done()) });
}

/// Draw the kit's gallery, with a sheet over it if `sheet`.
fn gallery(gpu: &engine::Gpu, runtime: &tokio::runtime::Runtime, sheet: bool) -> image::RgbaImage {
	let render = render_state(gpu);
	let mut harness = Harness::builder()
		.with_size(SIZE)
		.renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(render.clone()))
		.build_eframe(|cc| {
			crate::kit::install(&cc.egui_ctx);
			let mut fx = fx::Fx::new(&render, None, &cc.egui_ctx, runtime.handle());
			fx.pick(Effects::Full);
			crate::kit::gallery::Gallery::new(fx, sheet)
		});
	// Enough for the popover and the sheet to settle where they open.
	harness.run_steps(4);
	harness.render().unwrap()
}

/// Mute the first sound, open the health dot's popover, and rest the pointer
/// on undo until its tooltip shows.
fn hover(harness: &mut Harness<'_, Window>) {
	harness
		.get_all_by_label("Mute")
		.next()
		.expect("a sound to mute")
		.click();
	harness.run_steps(2);
	harness.get_by_label("No relay").click();
	harness.run_steps(2);
	harness.get_by_label_contains("Undo: Mute").hover();
	// egui shows a tooltip once the pointer rests half a second.
	harness.run_steps(4);
}

/// The library's shows: a few recent ones, beside the view's directory, most
/// with the person's picture as their last program.
fn shelf() -> (App, library::Recent) {
	let pictures = std::path::absolute("pictures").unwrap();
	let person = image::open("person.jpg").unwrap().to_rgba8();
	let day = 24 * 3600;
	let now = library::now();
	let shows = [
		("Friday Night Ranked", Some((3 * day, 3 * 3600 + 120)), Some(false)),
		("Sunday Chill", Some((6 * day, 6000)), Some(false)),
		("Garage Sale Live", Some((14 * day, 55 * 60)), Some(true)),
		("Podcast with Sam", None, None),
		("Workbench Close-ups", Some((40 * day, 2 * 3600 + 900)), Some(false)),
	];
	let mut recent = library::Recent::default();
	// Oldest first, so the first is the latest opened.
	for (index, (name, live, picture)) in shows.into_iter().enumerate().rev() {
		let path = std::path::absolute(format!("{name}.toml")).unwrap();
		std::fs::write(&path, "").unwrap();
		recent.opened(&path, now - 21 * day + (5 - index as u64) * 3600);
		if let Some((ago, lasted)) = live {
			recent.aired(&path, library::Aired { at: now - ago, lasted });
		}
		if let Some(portrait) = picture {
			let (width, height) = if portrait { (360, 640) } else { (640, 360) };
			let picture = image::imageops::resize(&person, width, height, image::imageops::FilterType::Triangle);
			let picture = library::Picture {
				width,
				height,
				rgba: picture.into_raw(),
			};
			recent.shaped(&path, portrait);
			library::save_picture(&pictures, &path, &picture).unwrap();
		}
	}
	let app = App::Library(Box::new(library::Library::new(Some(pictures))), None);
	(app, recent)
}

/// Draw until the go-live sheet's checks finish.
async fn checks(harness: &mut Harness<'_, Window>) {
	loop {
		harness.step();
		let App::Studio(studio) = &harness.state().app else {
			panic!("the studio stays open")
		};
		if studio.checked() {
			break;
		}
		kio::clock::sleep(Duration::from_millis(50)).await;
	}
	harness.run_steps(2);
}

/// Draw until setup has finished its checks and proposes a scene.
async fn checked(harness: &mut Harness<'_, Window>) {
	loop {
		harness.step();
		let App::Setup(setup, _) = &harness.state().app else {
			panic!("setup starts nothing on its own")
		};
		if setup.proposing() {
			break;
		}
		kio::clock::sleep(Duration::from_millis(20)).await;
	}
	harness.run_steps(2);
}

/// Wait until every source has started and the program has drawn a second of
/// frames since, enough to redraw every thumbnail the window shows, then draw.
///
/// The window is not drawn while it waits: egui's clock moves a step a frame,
/// and the effects with it, so a still view takes the same steps however
/// long the program takes.
async fn settle(harness: &mut Harness<'_, Window>, engine: &engine::Engine) {
	// The thumbnails and preview the view shows, which the compositor draws
	// only once a frame asks for them.
	harness.step();
	let mut since: Option<u64> = None;
	loop {
		let rendered = engine.stats.rendered.load(Ordering::Relaxed);
		let (started, framerate) = {
			let state = engine.state.lock().unwrap();
			let started = state.sources.iter().all(|(_, feed)| {
				!matches!(feed.status(), Status::Starting) && !matches!(feed.track(), Some(Status::Starting))
			});
			(started, u64::from(state.compositor.canvas().framerate))
		};
		match since {
			None if started => since = Some(rendered),
			Some(since) if rendered >= since + framerate => break,
			_ => {}
		}
		// The program draws on a clock of its own, at the canvas rate.
		kio::clock::sleep(Duration::from_millis(20)).await;
	}
	// Twice: once for egui to lay out what it learned, once to draw it.
	harness.run_steps(2);
}

/// A renderer on `gpu`'s device, as the window's is.
fn render_state(gpu: &engine::Gpu) -> egui_wgpu::RenderState {
	let config = egui_wgpu::WgpuConfiguration {
		wgpu_setup: egui_wgpu::WgpuSetup::Existing(egui_wgpu::WgpuSetupExisting {
			instance: gpu.instance.clone(),
			adapter: gpu.adapter.clone(),
			device: gpu.device.clone(),
			queue: gpu.queue.clone(),
		}),
		..Default::default()
	};
	pollster::block_on(egui_wgpu::RenderState::create(
		&config,
		&gpu.instance,
		None,
		egui_wgpu::RendererOptions::PREDICTABLE,
	))
	.unwrap()
}
