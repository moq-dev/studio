//! Named views of the window, rendered to PNG with no display: `just shots`.
//!
//! Each view is a test that opens the show in `shots/show.toml` on a fresh
//! engine and draws one state of the window through egui_kittest, on the GPU
//! the compositor uses. A still view draws the same picture every time, which
//! its test checks by drawing it twice; a live one shows something that moves,
//! such as an encoder's timings.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use eframe::egui_wgpu;
use egui_kittest::Harness;
use moq_scene::source::Status;

use crate::fx::Effects;
use crate::ui::Tab;
use crate::{App, Window, autosave, engine, fx, hotkeys, setup};

/// The window's size, the mock-ups'.
const SIZE: egui::Vec2 = egui::vec2(1440.0, 900.0);

/// The longest a view waits for its sources and pictures to settle.
const SETTLE: Duration = Duration::from_secs(20);

/// What a view opens.
enum Open {
	/// Guided setup, as the first launch shows it.
	Setup,
	/// The studio, with a panel at the front of its group.
	Studio(Option<Tab>),
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

// Every view. A quest that adds a page or a state adds its row.
views! {
	/// Once its checks are done: this machine's encoder timings, and the
	/// capture errors of a build without `capture`, which keeps the view off
	/// the host's devices.
	setup: Live, Open::Setup;
	studio: Still, Open::Studio(None);
	graph: Still, Open::Studio(Some(Tab::Graph));
	history: Still, Open::Studio(Some(Tab::History));
	devices: Still, Open::Studio(Some(Tab::Devices));
	guests: Still, Open::Studio(Some(Tab::Guests));
	keyframes: Still, Open::Studio(Some(Tab::Keyframes));
	replay: Still, Open::Studio(Some(Tab::Replay));
	health: Still, Open::Studio(Some(Tab::Health));
	audio: Still, Open::Studio(Some(Tab::Audio));
	/// The encoders' timings.
	output: Live, Open::Studio(Some(Tab::Output));
	hotkeys: Still, Open::Studio(Some(Tab::Hotkeys));
}

/// Draw the view `name` opens into `$SHOTS_DIR/<name>.png`, twice if it is
/// still, failing if the two differ.
fn shot(name: &str, motion: Motion, open: Open) {
	let out = std::env::var_os("SHOTS_DIR").expect("SHOTS_DIR names where the PNGs go; run `just shots`");
	// Absolute, before the change of directory below.
	let out = std::path::absolute(out).unwrap();
	std::fs::create_dir_all(&out).unwrap();

	// The show is opened as `show.toml` from a directory of its own, so the
	// status bar names it the same on every machine. nextest runs each test
	// in a process of its own, which the change of directory relies on.
	let show = tempfile::tempdir().unwrap();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	std::fs::copy(
		fixture.join("../moq-scene/src/test_data/person.jpg"),
		show.path().join("person.jpg"),
	)
	.unwrap();
	std::env::set_current_dir(show.path()).unwrap();

	let runtime = tokio::runtime::Runtime::new().unwrap();
	let _entered = runtime.enter();
	let gpu = pollster::block_on(engine::Gpu::new()).expect("a GPU");
	let image = shoot(&open, &gpu, &runtime, &fixture.join("shots/show.toml"));
	if motion == Motion::Still {
		let again = shoot(&open, &gpu, &runtime, &fixture.join("shots/show.toml"));
		assert!(image == again, "the still view `{name}` drew a different picture twice");
	}
	let path = out.join(format!("{name}.png"));
	// The window is opaque, and a quarter smaller without the alpha.
	image::DynamicImage::ImageRgba8(image).to_rgb8().save(&path).unwrap();
	println!("{}", path.display());
}

/// Draw what `open` opens on a fresh engine, from the scene file `show`.
fn shoot(open: &Open, gpu: &engine::Gpu, runtime: &tokio::runtime::Runtime, show: &Path) -> image::RgbaImage {
	let launch = engine::Launch {
		gpu: gpu.clone(),
		relays: Vec::new(),
		broadcast: "studio".into(),
		on_demand: false,
		recordings: ".".into(),
		room: None,
	};
	let app = match open {
		Open::Setup => App::Setup(Box::new(
			setup::Setup::new(launch, "show.toml".into(), runtime.handle().clone()).unwrap(),
		)),
		Open::Studio(_) => {
			// Afresh, without the history an earlier draw's edits left.
			std::fs::copy(show, "show.toml").unwrap();
			let _ = std::fs::remove_file("show.history.toml");
			let (autosave, loaded) = autosave::Autosave::open("show.toml".into()).unwrap();
			App::Starting(
				runtime
					.block_on(engine::Engine::start(launch, autosave, loaded))
					.unwrap(),
			)
		}
	};
	let render = render_state(gpu);
	let started = Arc::new(OnceLock::new());
	let mut harness = Harness::builder()
		.with_size(SIZE)
		.renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(render.clone()))
		.build_eframe(|cc| {
			let mut fx = fx::Fx::new(&render, None, &cc.egui_ctx, runtime.handle());
			// Not the system's reduced motion, which differs between machines.
			fx.pick(Effects::Full);
			Window {
				app,
				render: render.clone(),
				runtime: runtime.handle().clone(),
				started: started.clone(),
				keyboard: hotkeys::Keyboard::default(),
				fx,
			}
		});

	let &Open::Studio(tab) = open else {
		runtime
			.block_on(kio::clock::timeout(SETTLE, checked(&mut harness)))
			.expect("setup finishes its checks");
		return harness.render().unwrap();
	};
	harness.step();
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	if let Some(tab) = tab {
		studio.front(tab);
	}
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	let image = harness.render().unwrap();
	drop(harness);
	engine.recorder.close();
	engine.stop();
	runtime.block_on(async { tokio::join!(engine.close(), engine.recorder.done()) });
	image
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
