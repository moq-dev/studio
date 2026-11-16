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
use crate::ui::{Inspect, Page, Setting, Side};
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
	/// The go-live sheet, once its checks finish.
	Going,
	/// On air, at the LIVE page.
	OnAir,
	/// On air, with the pointer on the clock.
	EndStream,
	/// On air, with the health dot's popover open.
	OnAirHealth,
	/// REVIEW, with this show's recordings and clips, and if `picked`, the
	/// last 30 seconds held and a stretch picked. With `replays`, the show
	/// has two replay sources to pick between.
	Review {
		picked: bool,
		replays: bool,
	},
	/// A page in Studio mode, with `Interview` cued.
	Cued(Page),
	/// A scene, by name, open for editing off air on BUILD.
	OffAir(&'static str),
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
	/// Studio mode: the cued scene's pad turns amber, and NEXT docks on the
	/// live picture with TAKE and AUTO.
	live_studio: Still, studio(View::Cued(Page::Live));
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
	build_compact: Still, Open::Studio { view: BUILD, size: HALF, show: "show.toml", relay: false };
	/// BUILD in Studio mode edits the scene on air, with NEXT docked on it.
	build_studio: Still, studio(View::Cued(Page::Build));
	/// A scene open off air: the frame turns amber, and the live picture
	/// shrinks to a corner beside the banner that takes it live or goes back.
	/// Layers lists the scene's own, as always on top is drawn on program.
	build_off_air: Still, studio(View::OffAir("Break"));
	/// A scene that cannot go on air opens off air too, its Take it live
	/// dimmed, saying why under the pointer.
	build_off_air_refused: Still, studio(View::OffAir("Vertical"));
	/// Too narrow for the banner beside the live picture, it goes under it.
	build_off_air_compact: Still, Open::Studio { view: View::OffAir("Break"), size: HALF, show: "show.toml", relay: false };
	/// MIX holds the mixer, and a small live picture in the sound's place.
	mix: Still, studio(View::Page(Page::Mix));
	/// MIX in a stacked window, its live picture as tall as the sound's slot
	/// allows.
	mix_compact: Still, Open::Studio { view: View::Page(Page::Mix), size: HALF, show: "show.toml", relay: false };
	/// REVIEW off air: the program keeps no past until it goes live, which the
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
	/// Settings' Advanced tab: the scene graph.
	advanced: Still, studio(View::Setting(Setting::Advanced));
	/// The edit history, open under undo.
	history: Still, studio(View::History);
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

	harness.get_by_label_contains("Undo: Panic: ").click();
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
	let (mut harness, started, _) = window(
		start(launch(&gpu), stream(None), &runtime),
		&gpu,
		&runtime,
		SIZE,
		Default::default(),
	);
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

/// Studio mode, driven as a person does: its switch turns it on, a pad then
/// cues its scene rather than taking it, and TAKE puts it on air. NEXT's Edit
/// opens the cued scene off air on BUILD, Back to live keeps it cued, and
/// the card's close clears it. Off again, a pad's menu opens a scene off air,
/// whose own layers BUILD lists and selects, and the banner's Take it live
/// takes it, keeping the selection on the scene now live. It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn studio_mode_cues_and_takes() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	std::fs::copy(fixture.join("shots/show.toml"), "show.toml").unwrap();
	let (mut harness, started, _) = window(
		start(launch(&gpu), stream(None), &runtime),
		&gpu,
		&runtime,
		SIZE,
		Default::default(),
	);
	harness.step();
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	// The scene on program, and the one cued.
	let staged = || {
		let state = engine.state.lock().unwrap();
		let program = state.compositor.program_scene().map(str::to_string);
		(program, state.cued().map(str::to_string))
	};
	let scenes = |program: &str, cued: Option<&str>| (Some(program.to_string()), cued.map(str::to_string));
	let selected = |harness: &mut Harness<'_, Window>| {
		let App::Studio(studio) = &mut harness.state_mut().app else {
			unreachable!()
		};
		studio.selected()
	};
	assert_eq!(staged(), scenes("Main", None));

	harness.get_by_label("Studio mode").click();
	harness.run_steps(2);
	harness.get_by_label("Interview").click();
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Main", Some("Interview")), "the pad cued its scene");
	harness.get_by_label("TAKE").click();
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Interview", None), "TAKE put it on air");

	harness.get_by_label("Main").click();
	harness.run_steps(2);
	harness.get_by_label("Edit").click();
	harness.run_steps(2);
	harness.get_by_label("Back to live").click();
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Interview", Some("Main")), "Studio mode keeps it cued");
	harness.get_by_label("Clear the cue").click();
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Interview", None));

	harness.get_by_label("Studio mode").click();
	harness.run_steps(2);
	harness.get_by_label("Main").click_secondary();
	harness.run_steps(2);
	harness.get_by_label("Edit off air").click();
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Interview", Some("Main")), "open off air, not taken");
	// BUILD lists the layers of the scene open off air, and
	// leaves always on top to the program.
	harness.get_by_label_contains("Always on top is drawn on program");
	harness.get_all_by_label("bar").next().unwrap().click();
	harness.run_steps(2);
	assert!(selected(&mut harness), "the click selected Main's bar off air");
	harness.get_by_label("Take it live").click();
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Main", None), "taken live");
	assert!(selected(&mut harness), "the bar stays selected, its scene now live");

	drop(harness);
	quit(&engine, &runtime);
}

/// Draw what `open` opens on a fresh engine, from a scene file in `shows`.
fn shoot(open: &Open, gpu: &engine::Gpu, runtime: &tokio::runtime::Runtime, shows: &Path) -> image::RgbaImage {
	if let &Open::Kit { sheet } = open {
		return gallery(gpu, runtime, sheet);
	}
	if let Open::Library = open {
		let (app, recent) = library_shelf();
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
		Open::Studio { show, view, relay, .. } => {
			let mut text = std::fs::read_to_string(shows.join(show)).unwrap();
			if let View::Review { replays: true, .. } = view {
				text.push_str("\n[sources.replay]\nkind = \"replay\"\n\n[sources.slowmo]\nkind = \"replay\"\n");
			}
			std::fs::write("show.toml", text).unwrap();
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
		View::Build { side, inspect, index } => studio.build(*side, *inspect, *index),
		View::Menu => studio.build(Side::Layers, Inspect::Layer, Some(1)),
		View::Setting(setting) => studio.open(*setting),
		View::Health => studio.details(),
		View::Going => studio.going(),
		View::Hover | View::History | View::OnAir | View::EndStream | View::OnAirHealth => {}
		View::Cued(page) => {
			studio.studio_mode(true);
			studio.cue("Interview");
			studio.turn(*page);
		}
		View::OffAir(scene) => studio.edit_off_air(scene),
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
fn library_shelf() -> (App, library::Recent) {
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
