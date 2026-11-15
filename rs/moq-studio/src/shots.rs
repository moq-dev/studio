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
use moq_scene::Source;
use moq_scene::source::Status;

use crate::fx::Effects;
use crate::ui::{Page, Setting, Tool};
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
	/// The BUILD page, showing a tool.
	Tool(Tool),
	/// Settings, at a tab.
	Setting(Setting),
	/// The stream's health in detail.
	Health,
	/// The LIVE page after a mute, with the health dot's popover open and the
	/// pointer on undo, which names the mute.
	Hover,
	/// REVIEW, with this show's recordings and clips, and if `picked`, the
	/// last 30 seconds held and a stretch picked. With `replays`, the show
	/// has two replay sources to pick between.
	Review {
		picked: bool,
		replays: bool,
	},
}

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
	build: Still, studio(View::Page(Page::Build));
	/// MIX holds the mixer, and a small live picture in the sound's place.
	mix: Still, studio(View::Page(Page::Mix));
	/// MIX in a stacked window, its live picture as tall as the sound's slot
	/// allows.
	mix_compact: Still, Open::Studio { view: View::Page(Page::Mix), size: HALF, show: "show.toml" };
	/// REVIEW off air: without a relay the program keeps no past, which the
	/// strip and both actions say, beside this show's recordings and clips.
	review: Still, studio(View::Review { picked: false, replays: false });
	/// REVIEW with the last 30 seconds held: a stretch picked between the
	/// handles, the head inside it, the picture under the head large, and
	/// Replay saying why it cannot play without a replay source.
	review_picked: Still, studio(View::Review { picked: true, replays: false });
	/// REVIEW on a show with two replay sources: which one Replay plays in is
	/// picked under the speed, in place of its note.
	review_into: Still, studio(View::Review { picked: true, replays: true });
	/// Settings' Recording tab: the folder, and what a recording holds.
	recording: Still, studio(View::Setting(Setting::Recording));
	graph: Still, studio(View::Setting(Setting::Graph));
	history: Still, studio(View::Setting(Setting::History));
	devices: Still, studio(View::Tool(Tool::Devices));
	guests: Still, studio(View::Tool(Tool::Guests));
	keyframes: Still, studio(View::Tool(Tool::Keyframes));
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
		Open::Studio { show, view, .. } => {
			let mut text = std::fs::read_to_string(shows.join(show)).unwrap();
			if let View::Review { replays: true, .. } = view {
				text.push_str("\n[sources.replay]\nkind = \"replay\"\n\n[sources.slowmo]\nkind = \"replay\"\n");
			}
			std::fs::write("show.toml", text).unwrap();
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
		View::Tool(tool) => studio.tool(*tool),
		View::Setting(setting) => studio.open(*setting),
		View::Health => studio.details(),
		View::Hover => {}
		View::Review { picked, .. } => {
			studio.turn(Page::Review);
			studio.seed_shelf(shelf());
			if *picked {
				let secs = Duration::from_secs;
				let now = kio::clock::Instant::now();
				studio.seed_past(past(now), now, (secs(14), secs(6)), secs(9));
			}
		}
	}
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	if let View::Hover = view {
		hover(&mut harness);
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

/// The last 30 seconds as of `now`: a keyframe every 2.5 s, a sun setting
/// across a gradient, drawn rather than decoded so the view is the same on
/// every machine.
fn past(now: kio::clock::Instant) -> crate::past::View {
	let (width, height) = (480usize, 270usize);
	let pictures = (0..12u64)
		.map(|index| {
			let mut rgba = vec![0u8; width * height * 4];
			let sun = (60.0 + index as f32 * 30.0, 70.0 + index as f32 * 6.0);
			for (at, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
				let (x, y) = ((at % width) as f32, (at / width) as f32);
				let sky = y / height as f32;
				let mut color = [
					(40.0 + 160.0 * sky) as u8,
					(20.0 + 40.0 * sky) as u8,
					(90.0 - 40.0 * sky) as u8,
				];
				if (x - sun.0).powi(2) + (y - sun.1).powi(2) < 45.0_f32.powi(2) {
					color = [255, 170, 80];
				}
				pixel.copy_from_slice(&[color[0], color[1], color[2], 255]);
			}
			Arc::new(crate::past::Picture {
				id: index + 1,
				at: now.saturating_sub(Duration::from_millis(29_000 - index * 2_500)),
				size: [width, height],
				rgba,
			})
		})
		.collect::<Vec<_>>();
	// A packet each 20 ms, swelling and fading as speech does.
	let sound = (0..1450u64)
		.map(|index| {
			let at = now.saturating_sub(Duration::from_millis(29_000 - index * 20));
			let t = index as f32 / 50.0;
			let peak = ((t * 1.3).sin() * (t * 0.17).cos() * (t * 0.049 + 1.0).sin()).abs();
			(at, peak)
		})
		.collect();
	crate::past::View {
		of: crate::replay::Of::Program,
		held: Some(Ok(crate::past::Held {
			oldest: pictures[0].at,
			video: true,
		})),
		pictures,
		sound: Ok(sound),
	}
}

/// This show's recordings and clips, as the mock-up lists them.
fn shelf() -> crate::shelf::Listing {
	use crate::shelf::{Card, Kind, Listing};
	let wall = kio::clock::wall();
	let ago = |seconds: u64| Some(wall - Duration::from_secs(seconds));
	let length = |seconds: u64| Some(Duration::from_secs(seconds));
	let recording = |sources, repaired| Kind::Recording {
		stamp: String::new(),
		sources,
		repaired,
	};
	let card = |kind, modified, bytes, length| Card {
		kind,
		path: "show.mp4".into(),
		modified,
		bytes,
		length,
	};
	Listing {
		cards: vec![
			card(Kind::Clip(3), ago(150), 11_000_000, length(8)),
			card(Kind::Clip(2), ago(41 * 60), 18_000_000, length(12)),
			card(
				recording(0, true),
				ago(2 * 86_400 + 600),
				14_200_000_000,
				length(2 * 3600 + 10 * 60 + 44),
			),
			card(
				recording(4, false),
				ago(6 * 86_400 + 600),
				21_000_000_000,
				length(3 * 3600 + 2 * 60 + 10),
			),
			card(Kind::Clip(1), ago(6 * 86_400 + 900), 22_000_000, length(15)),
		],
		error: None,
		free: Some(212_000_000_000),
	}
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
			// A replay source's track starts with its first replay, so one that
			// idles has nothing to wait for.
			let started = state.sources.iter().all(|(_, feed)| {
				!matches!(feed.status(), Status::Starting)
					&& (matches!(feed.source(), Source::Replay) || !matches!(feed.track(), Some(Status::Starting)))
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
