//! `--watch`: play a studio's [composition](moq_scene::composition) as a
//! viewer would: the program, with each broadcast the composition lists
//! composited over it and mixed with it, on the program's media clock.
//!
//! A prototype for settling the composition's format before hang specifies
//! one, built on moq-scene: a collection the composition drives, rather than a
//! scene file, whose one scene draws the program, a [`Source::Broadcast`],
//! under each picture's [`Source::Layer`]. Each layer plays its own
//! broadcast on the program's playout clock, its timestamps moved by the
//! offset the composition gives it, each composition takes effect when the
//! program reaches the media time it is stamped with, and its pictures'
//! keyframes play from its `start` on that clock. A composition of another
//! version, or one this player cannot place, is refused loudly, and the
//! program plays alone.

use std::collections::{BTreeSet, VecDeque};
use std::time::Duration;

use anyhow::Context;
use kio::clock::Instant;
use moq_mux::timeline;
use moq_scene::composition::{self, Composition};
use moq_scene::source::{self, MAX_DELAY, Sources, Status};
use moq_scene::{AudioScene, Canvas, Collection, Compositor, Item, Rect, Scene, Source};

use tokio::sync::watch;

use crate::engine::{self, Gpu};
use crate::mix;

/// The program's source in the collection.
const PROGRAM: &str = "program";

/// What each layer's source name starts with, so none can take the
/// program's.
const LAYER: &str = "layer/";

/// The scene on program.
const SCENE: &str = "watch";

/// The canvas until the program's catalog or a composition gives its size.
const CANVAS: Canvas = Canvas {
	width: 1280,
	height: 720,
	framerate: 30,
};

/// Play the composition of the studio broadcast at `broadcast` on the relay
/// `client` dials at `url`, until the window closes.
pub fn run(
	client: moq_tokio::connect::Config,
	quic: moq_tokio::quic::Config,
	url: url::Url,
	broadcast: &str,
) -> anyhow::Result<()> {
	let program = moq_net::Path::new(broadcast).context("invalid --broadcast")?;
	let at = program.join(&moq_net::Path::from_static(composition::PATH))?;
	let runtime = tokio::runtime::Runtime::new()?;
	let _runtime = runtime.enter();
	let client = client.init(quic)?;
	let origin = moq_tokio::origin::spawn();
	tracing::info!(url = %moq_tokio::RedactedUrl::new(&url), composition = %at, "watching");
	let connection = client.with_subscriber(origin.clone()).connect(url.clone());

	let gpu = pollster::block_on(Gpu::new())?;
	let (bus, driver) = moq_audio::bus::Bus::new(Default::default())?;
	let config = bus.config().clone();
	let start = Instant::now();
	let (mixer, mixed) = source::Mixer::new(driver, start);
	engine::clocked("audio".into(), async move || advance(start, config, mixer).await)?;
	let monitor = match mix::Monitor::start(runtime.handle(), AudioScene::DEFAULT, None) {
		Ok(monitor) => Some(monitor),
		Err(err) => {
			tracing::warn!(error = format!("{err:#}"), "playing without sound");
			None
		}
	};

	let sources = Sources::new(source::Context {
		bus,
		mixed,
		origin: origin.consume(),
		dir: std::env::current_dir()?,
		output: moq_video::render::output(&gpu.device),
		publish: None,
		browser: None,
	});
	let paths = Paths { program, at };
	let collection = collection(None, CANVAS, &paths)?;
	let mut compositor = Compositor::new(&gpu.device, &gpu.queue, collection.canvas)?;
	compositor.set_previewing(false);
	compositor.set_preview(Some(SCENE.into()));
	compositor.cut(&collection, &sources)?;

	let (read, _) = watch::channel(Read::default());
	runtime.spawn(follow(origin.consume(), paths.at.clone(), read.clone()));
	runtime.spawn(size(origin.consume(), paths.program.clone(), read.clone()));

	let watch = Watch {
		url: format!("{} at {}", moq_tokio::RedactedUrl::new(&url), paths.program),
		read,
		paths,
		collection,
		synced: false,
		composition: None,
		applied: false,
		size: None,
		sources,
		compositor,
		monitor,
		shown: None,
	};
	let options = eframe::NativeOptions {
		viewport: egui::ViewportBuilder::default()
			.with_title("kixel.live Studio")
			.with_app_id("moq-studio")
			.with_inner_size([1280.0, 720.0]),
		wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
			// The compositor's program reaches the screen without leaving the GPU.
			wgpu_setup: eframe::egui_wgpu::WgpuSetup::Existing(eframe::egui_wgpu::WgpuSetupExisting {
				instance: gpu.instance,
				adapter: gpu.adapter,
				device: gpu.device,
				queue: gpu.queue,
			}),
			..Default::default()
		},
		..Default::default()
	};
	let ran = eframe::run_native(
		"moq-studio-watch",
		options,
		Box::new(move |cc| {
			let render = cc
				.wgpu_render_state
				.clone()
				.context("the window is not drawn with wgpu")?;
			Ok(Box::new(Window { render, watch }))
		}),
	);
	runtime.block_on(async {
		let _ = connection.close().await;
	});
	ran.map_err(|err| anyhow::anyhow!("the window failed: {err}"))
}

/// Advance the bus on the wall clock from `start`, so each mix plays.
async fn advance(start: Instant, config: moq_audio::bus::Config, mut mixer: source::Mixer) {
	let rate = u128::from(config.sample_rate);
	let mut clock = kio::clock::interval_at(start, engine::AUDIO_PERIOD);
	clock.set_missed_tick_behavior(kio::clock::MissedTickBehavior::Skip);
	let mut written: u128 = 0;
	loop {
		let tick = clock.tick().await;
		let due = tick.saturating_duration_since(start).as_nanos() * rate / 1_000_000_000;
		let frames = (due - written) as usize;
		if frames > 0 {
			mixer.advance(frames);
			written = due;
		}
	}
}

/// Where the watched broadcasts are, from the connection's root.
struct Paths {
	/// The program's.
	program: moq_net::Path,
	/// The composition's, which its references are relative to.
	at: moq_net::Path,
}

/// What the program's catalog and the composition track say, for the window
/// to apply.
#[derive(Default)]
struct Read {
	/// The program's picture size, from its catalog.
	size: Option<(u32, u32)>,
	/// Each composition read, or `None` for the program alone, and the media
	/// time it takes effect at, or `None` for at once, oldest first.
	pending: VecDeque<(Option<Composition>, Option<moq_net::Timestamp>)>,
	/// Why the composition was refused, so the program plays alone.
	refused: Option<String>,
}

impl Read {
	/// Forget what an instance of the composition said, as another replaces
	/// it or it goes offline: its stamps are not the next one's.
	fn reset(&mut self) {
		self.refused = None;
		self.pending.clear();
		self.pending.push_back((None, None));
	}

	/// Queue `composition` to take effect at `at`, or at once for `None`,
	/// which supersedes every composition still waiting so a stalled program
	/// cannot hold them up. The program plays at most [`MAX_DELAY`] behind
	/// what arrives, so of the compositions stamped that far before it, all
	/// due by now, only the newest is kept.
	fn push(&mut self, composition: Option<Composition>, at: Option<moq_net::Timestamp>) {
		match at {
			None => self.pending.clear(),
			Some(at) => {
				let passed = self.pending.iter().take_while(|(_, stamp)| {
					stamp.is_none_or(|stamp| Duration::from(stamp) + MAX_DELAY <= Duration::from(at))
				});
				self.pending.drain(..passed.count().saturating_sub(1));
			}
		}
		self.pending.push_back((composition, at));
	}

	/// Take the newest composition due at `now`: each takes effect once the
	/// program plays the frame it is stamped with on `schedule`, and its sound
	/// has played. `None` while none is due; `Some(None)` for the program
	/// alone.
	fn take_due(&mut self, schedule: Option<&timeline::Consumer>, now: Instant) -> Option<Option<Composition>> {
		let mut taken = None;
		while let Some((_, at)) = self.pending.front() {
			let due = match at {
				None => true,
				Some(at) => schedule.is_some_and(|schedule| {
					schedule.due(*at).is_some_and(|due| due <= now) && source::heard(schedule, *at, now)
				}),
			};
			if !due {
				break;
			}
			taken = self.pending.pop_front().map(|(composition, _)| composition);
		}
		taken
	}
}

/// Read the program's size from each catalog of each instance of the
/// broadcast at `path` as it comes online.
async fn size(origin: moq_net::origin::Consumer, path: moq_net::Path, read: watch::Sender<Read>) {
	let mut instances = match origin.follow(path, None) {
		Ok(instances) => instances,
		Err(err) => {
			tracing::error!(%err, "cannot follow the program");
			return;
		}
	};
	let set = |size| read.send_if_modified(|read| std::mem::replace(&mut read.size, size) != size);
	let mut playing = None;
	loop {
		tokio::select! {
			event = instances.available() => match event {
				Ok(Some(broadcast)) => playing = Some(Box::pin(catalogs(broadcast, set))),
				Ok(None) => {
					set(None);
					playing = None;
				}
				Err(err) => {
					tracing::error!(%err, "the program's announcements failed");
					return;
				}
			},
			result = async { playing.as_mut().expect("guarded").await }, if playing.is_some() => {
				playing = None;
				if let Err(err) = result {
					tracing::warn!(error = format!("{err:#}"), "cannot read the program's catalog");
				}
			}
		}
	}
}

/// Hand `set` the picture size of each catalog of `broadcast`.
async fn catalogs(
	broadcast: moq_net::broadcast::Consumer,
	set: impl Fn(Option<(u32, u32)>) -> bool,
) -> anyhow::Result<()> {
	let mut catalogs = moq_mux::catalog::Consumer::<()>::new(&broadcast, Default::default()).await?;
	while let Some(catalog) = moq_mux::catalog::Stream::next(&mut catalogs).await? {
		let size = catalog
			.video
			.renditions
			.values()
			.find_map(|config| config.coded_width.zip(config.coded_height));
		set(size);
	}
	Ok(())
}

/// Read each instance of the composition broadcast at `path` as it comes
/// online.
async fn follow(origin: moq_net::origin::Consumer, path: moq_net::Path, read: watch::Sender<Read>) {
	let mut instances = match origin.follow(path, None) {
		Ok(instances) => instances,
		Err(err) => {
			tracing::error!(%err, "cannot follow the composition");
			return;
		}
	};
	let mut playing = None;
	loop {
		tokio::select! {
			event = instances.available() => match event {
				Ok(Some(broadcast)) => {
					read.send_modify(Read::reset);
					playing = Some(Box::pin(instance(broadcast, read.clone())));
				}
				Ok(None) => {
					read.send_modify(Read::reset);
					playing = None;
				}
				Err(err) => {
					tracing::error!(%err, "the composition's announcements failed");
					return;
				}
			},
			result = async { playing.as_mut().expect("guarded").await }, if playing.is_some() => {
				playing = None;
				if let Err(err) = result {
					tracing::warn!(error = format!("{err:#}"), "cannot read the composition");
				}
			}
		}
	}
}

/// Read one instance of the composition broadcast until its catalog ends:
/// each composition while the catalog lists its track.
async fn instance(broadcast: moq_net::broadcast::Consumer, read: watch::Sender<Read>) -> anyhow::Result<()> {
	let mut catalogs = moq_mux::catalog::Consumer::<()>::new(&broadcast, Default::default()).await?;
	let mut compositions: Option<moq_json::snapshot::Consumer<Composition>> = None;
	// A refused composition stays refused for this instance.
	let mut refused = false;
	let alone = || read.send_modify(|read| read.push(None, None));
	loop {
		tokio::select! {
			catalog = moq_mux::catalog::Stream::next(&mut catalogs) => {
				let Some(catalog) = catalog? else {
					alone();
					return Ok(());
				};
				let listed = catalog.json.tracks.contains_key(composition::TRACK);
				if listed && compositions.is_none() && !refused {
					let track = broadcast.track(composition::TRACK)?;
					compositions = Some(moq_json::snapshot::Consumer::new(track, Default::default()));
				} else if !listed && compositions.take().is_some() {
					alone();
				}
			}
			next = async { compositions.as_mut().expect("guarded").next().await }, if compositions.is_some() => match next {
				Ok(Some(timed)) => read.send_modify(|read| read.push(Some(timed.value), timed.at)),
				Ok(None) => {
					compositions = None;
					alone();
				}
				Err(err) => {
					tracing::error!(%err, "refusing the composition; playing the program alone");
					compositions = None;
					refused = true;
					read.send_modify(|read| {
						read.refused = Some(err.to_string());
						read.push(None, None);
					});
				}
			},
		}
	}
}

/// The collection `composition` drives on a canvas the program's `size` when
/// it has none: the program at `paths`, and over it each picture's layer,
/// with the program and each child's sound in the audio scene on program.
/// Refused when it lists the program elsewhere, references a broadcast above
/// the connection's root, or places a broadcast it does not list.
fn collection(composition: Option<&Composition>, size: Canvas, paths: &Paths) -> anyhow::Result<Collection> {
	let mut collection = Collection::new();
	collection.canvas = match composition {
		Some(composition) => Canvas {
			width: composition.canvas.width,
			height: composition.canvas.height,
			framerate: size.framerate,
		},
		None => size,
	};
	collection.sources.insert(
		PROGRAM.into(),
		Source::Broadcast {
			path: paths.program.to_string(),
		},
	);
	let mut items = vec![Item::new(PROGRAM, Rect::fill(&collection.canvas))];
	let mut sounds = BTreeSet::from([PROGRAM.to_string()]);
	let layer = |source: &str| format!("{LAYER}{source}");
	if let Some(composition) = composition {
		let program = paths.at.try_resolve(&composition.program.broadcast);
		anyhow::ensure!(
			program.as_ref() == Some(&paths.program),
			"the composition's program is at `{}`, not the watched `{}`",
			composition.program.broadcast.as_str(),
			paths.program
		);
		for (name, child) in &composition.sources {
			let path = paths.at.try_resolve(&child.broadcast).with_context(|| {
				format!(
					"`{name}` is at `{}`, above the connection's root",
					child.broadcast.as_str()
				)
			})?;
			collection.sources.insert(
				layer(name),
				Source::Layer {
					clock: PROGRAM.into(),
					path: path.to_string(),
					instance: child.instance.clone(),
					offset: child.offset,
				},
			);
			if let Some(audio) = &child.sound {
				collection.audio.insert(layer(name), audio.clone());
				sounds.insert(layer(name));
			}
		}
		for picture in &composition.pictures {
			anyhow::ensure!(
				composition.sources.contains_key(&picture.source),
				"a picture shows `{}`, which the composition does not list",
				picture.source
			);
			items.push(Item {
				source: layer(&picture.source),
				..picture.clone()
			});
		}
	}
	collection.scenes = vec![Scene {
		items,
		..Scene::new(SCENE)
	}];
	collection.audio_scenes = vec![AudioScene {
		program: true,
		sounds,
		..AudioScene::new(AudioScene::DEFAULT)
	}];
	collection.validate().context("cannot play the composition")?;
	Ok(collection)
}

/// The player: the collection the composition drives, and what plays it.
struct Watch {
	/// What the window says it waits for.
	url: String,
	read: watch::Sender<Read>,
	paths: Paths,
	collection: Collection,
	/// Whether the sources run what `collection` holds.
	synced: bool,
	/// The composition in effect, or `None` for the program alone.
	composition: Option<Composition>,
	/// Whether `collection` holds the composition, which waits for the
	/// program stream it names.
	applied: bool,
	/// The program's size, from its catalog.
	size: Option<(u32, u32)>,
	sources: Sources,
	compositor: Compositor,
	monitor: Option<mix::Monitor>,
	/// The program as the window shows it.
	shown: Option<crate::ui::Registered>,
}

impl Watch {
	/// Apply what was read and is now due at `now`, and draw the program.
	fn render(&mut self, now: Instant) -> anyhow::Result<()> {
		let program = self.sources.get(PROGRAM);
		let schedule = program.and_then(|feed| feed.clock()).cloned();
		let playing = program.and_then(|feed| feed.instance());
		let mut changed = !self.synced;
		self.read.send_if_modified(|read| {
			if read.size != self.size {
				self.size = read.size;
				changed = true;
			}
			if let Some(composition) = read.take_due(schedule.as_ref(), now) {
				self.composition = composition;
				changed = true;
			}
			false
		});
		// Its offsets are for the program stream it names; one a relay minted
		// can neither match nor contradict it.
		let applies = self.composition.as_ref().is_some_and(|composition| {
			let named = composition.program.instance.as_ref();
			let playing = playing.as_ref().filter(|playing| !playing.is_minted());
			named.zip(playing).is_none_or(|(named, playing)| named == playing)
		});
		changed |= applies != self.applied;

		if changed {
			let size = match self.size {
				Some((width, height)) => Canvas {
					width,
					height,
					..CANVAS
				},
				None => CANVAS,
			};
			let composition = self.composition.as_ref().filter(|_| applies);
			let collection = match collection(composition, size, &self.paths) {
				Ok(collection) => collection,
				Err(err) => {
					tracing::error!(error = format!("{err:#}"), "playing the program alone");
					self.read.send_modify(|read| read.refused = Some(format!("{err:#}")));
					self.composition = None;
					collection(None, size, &self.paths)?
				}
			};
			self.applied = applies && self.composition.is_some();
			if collection.canvas != self.collection.canvas {
				self.compositor.fit(&collection)?;
			}
			self.collection = collection;
			for driver in self.sources.sync(&self.collection) {
				engine::spawn(driver);
			}
			if let (Some(monitor), Some(mix)) = (&self.monitor, self.sources.mix(AudioScene::DEFAULT)) {
				monitor.play(mix.clone());
			}
			self.synced = true;
		}

		// The keyframes play from `start` as far as the program has played,
		// following the clock as it re-anchors or stalls with the sound.
		if let (Some(composition), Some(schedule)) = (self.composition.as_ref().filter(|_| self.applied), &schedule) {
			let went_on = match schedule.playhead(now) {
				Some(head) => now.checked_sub(head.saturating_sub(composition.start)),
				None => moq_net::Timestamp::try_from(composition.start)
					.ok()
					.and_then(|start| schedule.due(start)),
			};
			if let Some(went_on) = went_on {
				self.compositor.set_program_start(went_on);
			}
		}

		self.compositor.render(&self.collection, &self.sources, now)?;
		Ok(())
	}

	/// What the window says over the program, if anything.
	fn status(&self) -> Option<String> {
		if let Some(refused) = &self.read.borrow().refused {
			return Some(format!("Playing the program alone: {refused}"));
		}
		match self.sources.get(PROGRAM).map(|feed| feed.status()) {
			Some(Status::Live) => None,
			Some(Status::Failed(err)) => Some(format!("{}: {err}", self.url)),
			_ => Some(format!("Waiting for {}", self.url)),
		}
	}
}

/// The window around the player.
struct Window {
	render: eframe::egui_wgpu::RenderState,
	watch: Watch,
}

impl eframe::App for Window {
	fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
		let watch = &mut self.watch;
		if let Err(err) = watch.render(kio::clock::now()) {
			tracing::error!(error = format!("{err:#}"), "cannot draw the program");
		}
		let texture = watch.compositor.program();
		match &mut watch.shown {
			Some(shown) => shown.follow(&self.render, texture),
			None => watch.shown = Some(crate::ui::Registered::new(&self.render, texture)),
		}
		let rect = ui.max_rect();
		let painter = ui.painter();
		painter.rect_filled(rect, 0.0, egui::Color32::BLACK);
		let canvas = watch.collection.canvas;
		let scale = (rect.width() / canvas.width as f32).min(rect.height() / canvas.height as f32);
		let fit = egui::Rect::from_center_size(
			rect.center(),
			egui::vec2(canvas.width as f32 * scale, canvas.height as f32 * scale),
		);
		let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
		let id = watch.shown.as_ref().expect("registered above").id;
		painter.image(id, fit, uv, egui::Color32::WHITE);
		if let Some(status) = watch.status() {
			painter.text(
				rect.left_top() + egui::vec2(12.0, 12.0),
				egui::Align2::LEFT_TOP,
				status,
				egui::FontId::proportional(16.0),
				egui::Color32::LIGHT_RED,
			);
		}
		ui.ctx().request_repaint();
	}
}

#[cfg(test)]
mod tests {
	use std::collections::BTreeMap;

	use moq_scene::composition::{Child, Program, Version};
	use moq_scene::{Audio, Filter, Keyframe, Size};

	use super::*;

	fn relative(path: &str) -> moq_net::path::Relative {
		moq_net::path::Relative::new(path).unwrap()
	}

	fn paths() -> Paths {
		Paths {
			program: moq_net::Path::new("show").unwrap(),
			at: moq_net::Path::new("show/composition").unwrap(),
		}
	}

	/// A camera named as the program is, placed twice, and a guest heard,
	/// each a broadcast of its own.
	fn composition(start: Duration) -> Composition {
		let mut cam = Item::new(
			"program",
			Rect {
				x: 10.0,
				y: 10.0,
				width: 64.0,
				height: 36.0,
			},
		);
		cam.filters = vec![Filter::Opacity { amount: 0.5 }];
		cam.keyframes = vec![Keyframe {
			x: 100.0,
			..Keyframe::new(Duration::from_secs(1))
		}];
		let mut guest = Child::new(relative("../guests/alice"), Some("abc".parse().unwrap()), -1_500);
		guest.sound = Some(Audio {
			gain: 0.5,
			delay: Duration::from_millis(80),
			..Default::default()
		});
		Composition {
			version: Version,
			canvas: Size {
				width: 640,
				height: 360,
			},
			start,
			program: Program {
				broadcast: relative("."),
				instance: None,
			},
			sources: BTreeMap::from([
				("program".into(), Child::new(relative("source/program"), None, 0)),
				("alice".into(), guest),
			]),
			pictures: vec![cam.clone(), cam],
		}
	}

	fn stamp(secs: u64) -> moq_net::Timestamp {
		moq_net::Timestamp::try_from(Duration::from_secs(secs)).unwrap()
	}

	/// The composition's pictures draw over the program from layers named
	/// apart from it, even one named as it is, each playing its own broadcast
	/// on the program's clock at its offset, a broadcast placed twice shares
	/// one layer, and the audio scene on program plays the program and each
	/// sound at its gain and delay.
	#[test]
	fn a_composition_drives_the_collection() {
		let composition = composition(Duration::ZERO);
		let driven = collection(Some(&composition), CANVAS, &paths()).unwrap();
		assert_eq!((driven.canvas.width, driven.canvas.height), (640, 360));
		let items = &driven.scene(SCENE).unwrap().items;
		let sources: Vec<&str> = items.iter().map(|item| item.source.as_str()).collect();
		assert_eq!(sources, [PROGRAM, "layer/program", "layer/program"]);
		assert_eq!(items[1].keyframes, composition.pictures[0].keyframes);
		assert_eq!(driven.sources[PROGRAM], Source::Broadcast { path: "show".into() });
		assert_eq!(
			driven.sources["layer/program"],
			Source::Layer {
				clock: PROGRAM.into(),
				path: "show/source/program".into(),
				instance: None,
				offset: 0,
			}
		);
		assert_eq!(
			driven.sources["layer/alice"],
			Source::Layer {
				clock: PROGRAM.into(),
				path: "guests/alice".into(),
				instance: Some("abc".parse().unwrap()),
				offset: -1_500,
			}
		);
		let heard = driven.heard(AudioScene::DEFAULT);
		assert_eq!(heard, BTreeSet::from([PROGRAM, "layer/alice"]));
		assert_eq!(
			Some(&driven.audio["layer/alice"]),
			composition.sources["alice"].sound.as_ref()
		);

		let alone = collection(None, CANVAS, &paths()).unwrap();
		assert_eq!(alone.sources.len(), 1, "the program alone");
		assert_eq!(alone.canvas, CANVAS);
	}

	/// A composition listing the program elsewhere, a broadcast above the
	/// connection's root, or a picture of a broadcast it does not list, is
	/// refused, saying why.
	#[test]
	fn refuses_what_it_cannot_place() {
		let refused = |edit: &dyn Fn(&mut Composition), why: &str| {
			let mut composition = composition(Duration::ZERO);
			edit(&mut composition);
			let err = format!("{:#}", collection(Some(&composition), CANVAS, &paths()).unwrap_err());
			assert!(err.contains(why), "{err}");
		};
		refused(
			&|composition| composition.program.broadcast = relative("other"),
			"not the watched `show`",
		);
		refused(
			&|composition| composition.sources.get_mut("alice").unwrap().broadcast = relative("../../../x"),
			"above the connection's root",
		);
		refused(
			&|composition| composition.pictures[0].source = "bob".into(),
			"which the composition does not list",
		);
	}

	/// Of the compositions the program must have reached by the time a new
	/// one arrives, only the newest waits.
	#[test]
	fn only_the_newest_passed_composition_waits() {
		let mut read = Read::default();
		for secs in [1, 2, 3, 4] {
			read.push(None, Some(stamp(secs)));
		}
		read.push(None, Some(stamp(5)));
		let stamps: Vec<_> = read.pending.iter().map(|(_, at)| *at).collect();
		assert_eq!(stamps, [Some(stamp(3)), Some(stamp(4)), Some(stamp(5))]);
	}

	/// Another instance of the composition starts from nothing: the last
	/// one's stamps and refusal are not its own.
	#[test]
	fn a_new_instance_forgets_the_last() {
		let mut read = Read::default();
		read.push(None, Some(stamp(1)));
		read.refused = Some("another version".into());
		read.reset();
		assert_eq!(read.pending.len(), 1);
		assert!(read.pending[0].0.is_none() && read.pending[0].1.is_none());
		assert!(read.refused.is_none());
	}

	/// Once the composition track is unlisted, the program plays alone at
	/// once, even while a stalled program holds back a composition still
	/// waiting.
	#[tokio::test(start_paused = true)]
	async fn unlisting_supersedes_a_waiting_composition() {
		let clock = timeline::Producer::new(Duration::from_millis(100));
		let audio = timeline::Audio {
			end: Duration::from_secs(1),
			buffered: Duration::ZERO,
			target: Duration::from_millis(100),
		};
		clock.audio(audio, Instant::now());
		let schedule = clock.consume();

		let (mut publisher, read, reader) = broadcast();
		publisher.update(composition(Duration::ZERO), stamp(5)).unwrap();
		pending(&read, 1).await;
		tokio::time::advance(Duration::from_secs(60)).await;
		let later = Instant::now();
		let take = || {
			let mut taken = None;
			read.send_if_modified(|read| {
				taken = read.take_due(Some(&schedule), later);
				false
			});
			taken
		};
		assert_eq!(take(), None, "the program stalled before it");

		publisher.finish().unwrap();
		alone(&read).await;
		assert_eq!(take(), Some(None), "the program alone");
		assert!(read.borrow().pending.is_empty());
		reader.abort();
	}

	/// A composition of another version, as a newer studio would publish.
	#[derive(serde::Serialize)]
	struct Newer {
		version: u32,
	}

	/// A composition broadcast, and the instance reader's view of it.
	fn broadcast() -> (
		composition::Publisher,
		watch::Sender<Read>,
		tokio::task::JoinHandle<anyhow::Result<()>>,
	) {
		let mut broadcast = moq_net::broadcast::Info::new().produce();
		let publisher = composition::Publisher::new(&mut broadcast, moq_mux::Clock::default()).unwrap();
		let (read, _) = watch::channel(Read::default());
		let reader = tokio::spawn(instance(broadcast.consume(), read.clone()));
		(publisher, read, reader)
	}

	/// What `read` holds once it has `count` compositions waiting, with their
	/// stamps as durations, since one read back carries its track's
	/// timescale.
	async fn pending(read: &watch::Sender<Read>, count: usize) -> Vec<(Option<Composition>, Option<Duration>)> {
		let mut reads = read.subscribe();
		let read = reads.wait_for(|read| read.pending.len() >= count).await.unwrap();
		let pending = read.pending.iter().cloned();
		pending
			.map(|(composition, at)| (composition, at.map(Duration::from)))
			.collect()
	}

	/// Wait until the newest thing `read` holds is the program alone.
	async fn alone(read: &watch::Sender<Read>) {
		let mut reads = read.subscribe();
		let alone = |read: &Read| read.pending.back().is_some_and(|(_, at)| at.is_none());
		reads.wait_for(alone).await.unwrap();
	}

	/// The reader hands over each composition at the media time it took
	/// effect, and the program alone once the composition ends.
	#[tokio::test]
	async fn reads_each_composition() {
		let (mut publisher, read, reader) = broadcast();
		publisher.update(composition(Duration::ZERO), stamp(1)).unwrap();
		let first = (Some(composition(Duration::ZERO)), Some(Duration::from_secs(1)));
		assert_eq!(pending(&read, 1).await, [first]);
		publisher.update(composition(Duration::from_secs(2)), stamp(2)).unwrap();
		let second = (Some(composition(Duration::from_secs(2))), Some(Duration::from_secs(2)));
		assert_eq!(pending(&read, 2).await[1], second);
		publisher.finish().unwrap();
		alone(&read).await;
		assert_eq!(pending(&read, 1).await, [(None, None)], "the program alone once ended");
		reader.abort();
	}

	/// A composition of another version is refused, saying why, and the
	/// program plays alone.
	#[tokio::test]
	async fn refuses_another_version() {
		let mut broadcast = moq_net::broadcast::Info::new().produce();
		let config = moq_mux::catalog::Config::default();
		let catalog = moq_mux::catalog::Producer::new(&mut broadcast, config).unwrap();
		let (read, _) = watch::channel(Read::default());
		let reader = tokio::spawn(instance(broadcast.consume(), read.clone()));
		let info = moq_net::track::Info::default().with_timescale(moq_mux::Clock::TIMESCALE);
		let track = broadcast.create_track(composition::TRACK, info).unwrap();
		let options = moq_mux::json::Options::default().with_deltas(false);
		let mut newer = catalog
			.json_snapshot::<Newer>(track, moq_mux::json::Config::default(), options)
			.unwrap();
		newer.update(&Newer { version: 3 }, stamp(1)).unwrap();
		assert_eq!(pending(&read, 1).await, [(None, None)]);
		let refused = read.borrow().refused.clone().unwrap();
		assert!(refused.contains("composition version 3 is not supported"), "{refused}");
		reader.abort();
	}
}
