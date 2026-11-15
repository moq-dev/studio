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
use crate::ui::{Inspect, Page, Setting, Side};
use crate::{App, Window, autosave, engine, fx, hotkeys, setup};

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
	/// The studio on the show in `shots/<show>`, in a window `size` large,
	/// turned to `view`.
	Studio {
		view: View,
		size: egui::Vec2,
		show: &'static str,
	},
	/// The kit's gallery, with a sheet open over it or not.
	Kit { sheet: bool },
}

/// Where a view turns the studio.
enum View {
	Page(Page),
	/// The BUILD page with `side` and `inspect` shown, and the layer of the
	/// scene on program at `index`, front first, selected.
	Build {
		side: Side,
		inspect: Inspect,
		index: Option<usize>,
	},
	/// BUILD with the host's layer selected and its row's menu open.
	Menu,
	/// The edit history, under the title bar's undo, after an edit.
	History,
	/// Settings, at a tab.
	Setting(Setting),
	/// The stream's health in detail.
	Health,
	/// The LIVE page after a mute, with the health dot's popover open and the
	/// pointer on undo, which names the mute.
	Hover,
}

/// BUILD with the host's layer, the middle of the scene on program's three,
/// selected, as the mock-up's.
const BUILD: View = View::Build {
	side: Side::Layers,
	inspect: Inspect::Layer,
	index: Some(1),
};

/// The studio on the shots' show, at the mock-ups' size, turned to `view`.
fn studio(view: View) -> Open {
	Open::Studio {
		view,
		size: SIZE,
		show: "show.toml",
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
	live_portrait: Still, Open::Studio { view: View::Page(Page::Live), size: SIZE, show: "portrait.toml" };
	/// A window snapped beside a game stacks the picture, the scenes, and the
	/// sound.
	live_compact: Still, Open::Studio { view: View::Page(Page::Live), size: HALF, show: "show.toml" };
	/// BUILD with the host's layer selected: its row in Layers, its handles
	/// and toolbar on the picture, and its inspector.
	build: Still, studio(BUILD);
	/// BUILD with no layer selected: the inspector shows the scene.
	build_scene: Still, studio(View::Build { side: Side::Layers, inspect: Inspect::Layer, index: None });
	/// The selected layer's keyframes.
	build_animate: Still, studio(View::Build { side: Side::Layers, inspect: Inspect::Animate, index: Some(1) });
	/// What can be added: sources no layer shows, devices, by hand, and
	/// guests.
	build_add: Still, studio(View::Build { side: Side::Add, inspect: Inspect::Layer, index: None });
	/// A layer row's menu, as a right click opens it.
	build_menu: Still, studio(View::Menu);
	/// BUILD in a window snapped beside a game: the inspector is a tab beside
	/// Layers and Add.
	build_compact: Still, Open::Studio { view: BUILD, size: HALF, show: "show.toml" };
	/// MIX holds the mixer, and a small live picture in the sound's place.
	mix: Still, studio(View::Page(Page::Mix));
	/// MIX in a stacked window, its live picture as tall as the sound's slot
	/// allows.
	mix_compact: Still, Open::Studio { view: View::Page(Page::Mix), size: HALF, show: "show.toml" };
	review: Still, studio(View::Page(Page::Review));
	/// Settings' Advanced tab: the scene graph.
	advanced: Still, studio(View::Setting(Setting::Advanced));
	/// The edit history, open under undo.
	history: Still, studio(View::History);
	health: Still, studio(View::Health);
	/// Settings' Show tab, with the encoders' timings.
	output: Live, studio(View::Setting(Setting::Show));
	hotkeys: Still, studio(View::Setting(Setting::Hotkeys));
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

/// How a view starts the studio: with no relay, recording beside the show.
fn launch(gpu: &engine::Gpu) -> engine::Launch {
	engine::Launch {
		gpu: gpu.clone(),
		relays: Vec::new(),
		broadcast: "studio".into(),
		on_demand: false,
		recordings: ".".into(),
		room: None,
	}
}

/// Start the studio by `launch` on `show.toml`, without the history an
/// earlier draw's edits left.
fn start(launch: engine::Launch, runtime: &tokio::runtime::Runtime) -> App {
	let _ = std::fs::remove_file("show.history.toml");
	let (autosave, loaded) = autosave::Autosave::open("show.toml".into()).unwrap();
	App::Starting(
		runtime
			.block_on(engine::Engine::start(launch, autosave, loaded))
			.unwrap(),
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
	let (mut harness, started, closed) = window(start(launch, &runtime), &gpu, &runtime, SIZE);
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

/// On BUILD, a layer row's menu hides, locks, reorders, and duplicates its
/// layer, a scene's pad's duplicates the scene with its own copies, and
/// undo takes each back, and a take lets go of a layer of the scene it
/// leaves. It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn a_layer_menu_edits_its_layer() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	std::fs::copy(fixture.join("shots/show.toml"), "show.toml").unwrap();
	let (mut harness, started, _) = window(start(launch(&gpu), &runtime), &gpu, &runtime, SIZE);
	harness.step();
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	studio.build(Side::Layers, Inspect::Layer, None);
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	let main = || {
		let state = engine.state.lock().unwrap();
		state.collection().scene("Main").unwrap().items.clone()
	};
	let menu = |harness: &mut Harness<'_, Window>, row: &str, button: &str| {
		harness.get_all_by_label(row).next().unwrap().click_secondary();
		harness.run_steps(2);
		// The menu's, which comes after the row's own eye and lock and the
		// inspector's buttons of the same names.
		harness.get_all_by_label(button).last().unwrap().click();
		harness.run_steps(2);
	};

	menu(&mut harness, "bar", "Hide");
	assert!(!main()[2].visible, "the menu hid the bar");
	menu(&mut harness, "bar", "Lock");
	assert!(main()[2].locked, "and locked it");
	menu(&mut harness, "bar", "Send to back");
	assert_eq!(main()[0].source, "bar", "it went to the back");
	menu(&mut harness, "host", "Duplicate as its own copy");
	let items = main();
	assert_eq!(items.len(), 4);
	assert_eq!(items[3].source, "host copy", "the copy went in front of the host");
	let cloned = engine.state.lock().unwrap().collection().sources["host copy"].clone();
	assert_eq!(cloned, moq_scene::Source::Clone { source: "host".into() });
	// The menu acts while it holds the engine's state, which copying reads.
	menu(&mut harness, "bar", "Copy");
	harness.get_by_label("copied `bar`");

	harness.get_by_label("Interview").click_secondary();
	harness.run_steps(2);
	harness.get_by_label("Duplicate with its own copies").click();
	harness.run_steps(2);
	{
		let state = engine.state.lock().unwrap();
		let collection = state.collection();
		let copy = collection.scene("Interview copy").expect("the scene was duplicated");
		let shown: Vec<&str> = copy.items.iter().map(|item| item.source.as_str()).collect();
		assert_eq!(shown, ["backdrop copy", "host copy 2", "guest copy"]);
	}

	for _ in 0..5 {
		harness.get_by_label_contains("Undo: ").click();
		harness.run_steps(2);
	}
	let items = main();
	let shown: Vec<&str> = items.iter().map(|item| item.source.as_str()).collect();
	assert_eq!(shown, ["backdrop", "host", "bar"], "undo took each edit back");
	assert!(items[2].visible && !items[2].locked);
	assert!(
		engine
			.state
			.lock()
			.unwrap()
			.collection()
			.scene("Interview copy")
			.is_none()
	);

	// A take moves BUILD to another scene, letting go of a layer of this one.
	let selected = |harness: &mut Harness<'_, Window>| {
		let App::Studio(studio) = &mut harness.state_mut().app else {
			unreachable!()
		};
		studio.selected()
	};
	harness.get_all_by_label("bar").next().unwrap().click();
	harness.run_steps(2);
	assert!(selected(&mut harness), "the click selected the bar");
	harness.get_by_label("Interview").click();
	harness.run_steps(2);
	assert!(!selected(&mut harness), "the take let go of Main's bar");

	drop(harness);
	quit(&engine, &runtime);
}

/// Draw what `open` opens on a fresh engine, from a scene file in `shows`.
fn shoot(open: &Open, gpu: &engine::Gpu, runtime: &tokio::runtime::Runtime, shows: &Path) -> image::RgbaImage {
	if let &Open::Kit { sheet } = open {
		return gallery(gpu, runtime, sheet);
	}
	let app = match open {
		Open::Setup => App::Setup(Box::new(
			setup::Setup::new(launch(gpu), "show.toml".into(), runtime.handle().clone()).unwrap(),
		)),
		Open::Kit { .. } => unreachable!("drawn without an engine"),
		Open::Studio { show, .. } => {
			std::fs::copy(shows.join(show), "show.toml").unwrap();
			start(launch(gpu), runtime)
		}
	};
	let size = match open {
		Open::Studio { size, .. } => *size,
		_ => SIZE,
	};
	let (mut harness, started, _) = window(app, gpu, runtime, size);

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
	match view {
		View::Page(page) => studio.turn(*page),
		View::Build { side, inspect, index } => studio.build(*side, *inspect, *index),
		View::Menu => studio.build(Side::Layers, Inspect::Layer, Some(1)),
		View::Setting(setting) => studio.open(*setting),
		View::Health => studio.details(),
		View::Hover | View::History => {}
	}
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	match view {
		View::Hover => hover(&mut harness),
		View::Menu => {
			// The row comes before the inspector's tab of the same name.
			harness.get_all_by_label("host").next().unwrap().click_secondary();
			harness.run_steps(4);
		}
		View::History => {
			harness
				.get_all_by_label("Mute")
				.next()
				.expect("a sound to mute")
				.click();
			harness.run_steps(2);
			harness.get_by_label("History").click();
			harness.run_steps(4);
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
) -> (Harness<'a, Window>, Arc<OnceLock<Arc<engine::Engine>>>, Arc<AtomicBool>) {
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
				runtime: runtime.handle().clone(),
				started: kept,
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
	harness.get_by_label("Not live").click();
	harness.run_steps(2);
	harness.get_by_label_contains("Undo: Mute").hover();
	// egui shows a tooltip once the pointer rests half a second.
	harness.run_steps(4);
}

/// Draw until setup has finished its checks and proposes a scene.
async fn checked(harness: &mut Harness<'_, Window>) {
	loop {
		harness.step();
		let App::Setup(setup) = &harness.state().app else {
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
