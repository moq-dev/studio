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
use moq_mux::sync;
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

	let gpu = pollster::block_on(Gpu::new(wgpu::Backends::all()))?;
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
		grants: None,
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
					stamp.is_none_or(|stamp| {
						matches!(
							(Duration::try_from(stamp), Duration::try_from(at)),
							(Ok(stamp), Ok(at)) if stamp + MAX_DELAY <= at
						)
					})
				});
				self.pending.drain(..passed.count().saturating_sub(1));
			}
		}
		self.pending.push_back((composition, at));
	}

	/// Take the newest composition due at `now`: each takes effect once the
	/// program plays the frame it is stamped with on `schedule`. `None` while
	/// none is due; `Some(None)` for the program alone.
	fn take_due(&mut self, schedule: Option<&sync::Consumer>, now: Instant) -> Option<Option<Composition>> {
		let mut taken = None;
		while let Some((_, at)) = self.pending.front() {
			let due = match at {
				None => true,
				Some(at) => schedule.is_some_and(|schedule| schedule.due(*at).is_some_and(|due| due <= now)),
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
/// each catalog's composition, or the program alone where it has none, to
/// take effect at the catalog's timestamp.
async fn instance(broadcast: moq_net::broadcast::Consumer, read: watch::Sender<Read>) -> anyhow::Result<()> {
	// Every update the program, playing up to `MAX_DELAY` behind, may still
	// reach, in order.
	let track = broadcast.track(hang::Catalog::DEFAULT_NAME)?;
	let mut catalogs = hang::catalog::Consumer::<composition::Ext>::new(track, MAX_DELAY);
	let refuse = |why: String| {
		tracing::error!(%why, "refusing the composition; playing the program alone");
		read.send_modify(|read| {
			read.refused = Some(why);
			read.push(None, None);
		});
	};
	loop {
		match catalogs.next().await {
			// A scale-less catalog says nothing of when a composition applies.
			Ok(Some(timed)) if timed.at.scale().is_none() => {
				refuse("its catalog is not timed".into());
				return Ok(());
			}
			Ok(Some(timed)) => read.send_modify(|read| read.push(timed.value.ext.composition, Some(timed.at))),
			Ok(None) => {
				read.send_modify(|read| read.push(None, None));
				return Ok(());
			}
			// The program plays alone from here, as at the catalog's end, rather
			// than under the last composition read for as long as the broadcast
			// stays announced.
			Err(hang::Error::Moq(err)) => {
				read.send_modify(|read| read.push(None, None));
				return Err(err.into());
			}
			// Refused for the rest of this instance.
			Err(err) => {
				refuse(err.to_string());
				return Ok(());
			}
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
				id: moq_scene::Id::fresh(),
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
			let mut collection = match collection(composition, size, &self.paths) {
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
			// A picture the last composition showed too is the same layer, so its
			// slot glides on from where it was.
			collection.recognize(&self.collection);
			self.collection = collection;
			// No undo here brings a picture back: one the composition dropped is
			// gone, so its parked placer is too.
			let shown = self.collection.layers().flat_map(|(_, items)| items);
			self.compositor.remember(shown.map(|item| item.id));
			for driver in self.sources.sync(&self.collection) {
				engine::spawn(driver);
			}
			if let (Some(monitor), Some(mix)) = (&self.monitor, self.sources.mix(AudioScene::DEFAULT)) {
				monitor.play(mix.clone());
			}
			self.synced = true;
		}

		// The keyframes play from when the program plays `start`, following
		// the clock as it moves.
		if let (Some(composition), Some(schedule)) = (self.composition.as_ref().filter(|_| self.applied), &schedule) {
			let went_on = moq_net::Timestamp::try_from(composition.start)
				.ok()
				.and_then(|start| schedule.due(start));
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

	/// Once the composition's catalog ends, the program plays alone at once,
	/// even while a program not playing yet holds back a composition still
	/// waiting.
	#[tokio::test(start_paused = true)]
	async fn an_ended_catalog_supersedes_a_waiting_composition() {
		let schedule = sync::Producer::new(Duration::from_millis(100)).consume();

		let (mut publisher, read, reader) = broadcast();
		publisher
			.update(composition(Duration::ZERO), &program(), stamp(5))
			.unwrap();
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
		assert_eq!(take(), None, "the program has no moment for it");

		publisher.finish().unwrap();
		alone(&read).await;
		assert_eq!(take(), Some(None), "the program alone");
		assert!(read.borrow().pending.is_empty());
		reader.abort();
	}

	/// The placer of a picture keeping clear that a composition dropped is
	/// forgotten once the next composition comes, since nothing can bring
	/// that picture back.
	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "needs a Vulkan device; run by just render-sw"]
	async fn a_dropped_picture_forgets_its_placer() {
		let gpu = Gpu::new(wgpu::Backends::all()).await.expect("a GPU");
		let (bus, driver) = moq_audio::bus::Bus::new(Default::default()).unwrap();
		let (_mixer, mixed) = source::Mixer::new(driver, Instant::now());
		let origin = moq_tokio::origin::spawn();
		let sources = Sources::new(source::Context {
			bus,
			mixed,
			origin: origin.consume(),
			dir: ".".into(),
			output: moq_video::render::output(&gpu.device),
			publish: None,
			grants: None,
			browser: None,
		});
		let collection = collection(None, CANVAS, &paths()).unwrap();
		let (read, _) = watch::channel(Read::default());
		let mut watch = Watch {
			url: String::new(),
			read,
			compositor: Compositor::new(&gpu.device, &gpu.queue, collection.canvas).unwrap(),
			paths: paths(),
			collection,
			synced: false,
			composition: None,
			applied: false,
			size: None,
			sources,
			monitor: None,
			shown: None,
		};
		let show = |watch: &mut Watch, composition: Option<Composition>| {
			watch.read.send_modify(|read| read.push(composition, None));
			watch.render(Instant::now()).unwrap();
		};

		let mut keeping = composition(Duration::ZERO);
		keeping.pictures.truncate(1);
		keeping.pictures[0].keep_clear = Some(moq_scene::KeepClear::default());
		show(&mut watch, Some(keeping.clone()));
		let item = &watch.collection.scene(SCENE).unwrap().items[1];
		let (id, rect) = (item.id, item.rect);
		assert!(watch.compositor.placement(id).is_some(), "it keeps clear");

		// Dropped, its placer is parked, then forgotten at the next composition.
		show(&mut watch, None);
		show(&mut watch, Some(keeping));
		assert_ne!(watch.collection.scene(SCENE).unwrap().items[1].id, id, "a new picture");
		watch.compositor.undone(id, rect, Instant::now());
		assert!(watch.compositor.placement(id).is_none(), "the placer is forgotten");
	}

	/// Once the composition's catalog fails, as when a relay drops its
	/// subscription, the program plays alone at once, as at its end.
	#[tokio::test(start_paused = true)]
	async fn a_failed_catalog_plays_the_program_alone() {
		let broadcast = moq_net::broadcast::Info::new().produce();
		let track = broadcast
			.create_track(hang::Catalog::DEFAULT_NAME, hang::Catalog::default_track_info())
			.unwrap();
		let mut catalog = moq_json::snapshot::Producer::new(track.clone(), Default::default());
		let (read, _) = watch::channel(Read::default());
		let reader = tokio::spawn(instance(broadcast.consume(), read.clone()));
		let mut first = hang::Catalog::<composition::Ext>::default();
		first.ext.composition = Some(composition(Duration::ZERO));
		catalog.update(moq_json::Timed::new(&first, stamp(1))).unwrap();
		pending(&read, 1).await;

		track.abort(moq_net::Error::Cancel).unwrap();
		alone(&read).await;
		assert!(reader.await.unwrap().is_err(), "a transport failure");
	}

	/// The catalog of a composition of another version, as a newer studio
	/// would publish.
	#[derive(serde::Serialize)]
	struct Newer {
		#[serde(rename = "live.kixel.studio.composition")]
		composition: Version3,
	}

	#[derive(serde::Serialize)]
	struct Version3 {
		version: u32,
	}

	/// The program's catalog, listing nothing.
	fn program() -> hang::Catalog {
		hang::Catalog::default()
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
			.map(|(composition, at)| (composition, at.map(|at| Duration::try_from(at).unwrap())))
			.collect()
	}

	/// Wait until the newest thing `read` holds is the program alone.
	async fn alone(read: &watch::Sender<Read>) {
		let mut reads = read.subscribe();
		let alone = |read: &Read| read.pending.back().is_some_and(|(_, at)| at.is_none());
		reads.wait_for(alone).await.unwrap();
	}

	/// The reader hands over each composition at the media time its catalog
	/// takes effect, and the program alone once the catalog ends.
	#[tokio::test]
	async fn reads_each_composition() {
		let (mut publisher, read, reader) = broadcast();
		publisher
			.update(composition(Duration::ZERO), &program(), stamp(1))
			.unwrap();
		let first = (Some(composition(Duration::ZERO)), Some(Duration::from_secs(1)));
		assert_eq!(pending(&read, 1).await, [first]);
		publisher
			.update(composition(Duration::from_secs(2)), &program(), stamp(2))
			.unwrap();
		let second = (Some(composition(Duration::from_secs(2))), Some(Duration::from_secs(2)));
		assert_eq!(pending(&read, 2).await[1], second);
		publisher.finish().unwrap();
		alone(&read).await;
		assert_eq!(pending(&read, 1).await, [(None, None)], "the program alone once ended");
		reader.abort();
	}

	/// Why the reader refuses the catalog `value` written at `at` on a track
	/// of `info`, once it plays the program alone.
	async fn refused(info: moq_net::track::Info, value: &impl serde::Serialize, at: moq_net::Timestamp) -> String {
		let broadcast = moq_net::broadcast::Info::new().produce();
		let track = broadcast.create_track(hang::Catalog::DEFAULT_NAME, info).unwrap();
		let mut catalog = moq_json::snapshot::Producer::new(track, Default::default());
		let (read, _) = watch::channel(Read::default());
		let reader = tokio::spawn(instance(broadcast.consume(), read.clone()));
		catalog.update(moq_json::Timed::new(value, at)).unwrap();
		assert_eq!(pending(&read, 1).await, [(None, None)]);
		reader.abort();
		read.borrow().refused.clone().unwrap()
	}

	/// A composition of another version is refused, saying why, and the
	/// program plays alone, as is one in a catalog that is not timed.
	#[tokio::test]
	async fn refuses_another_version() {
		let newer = Newer {
			composition: Version3 { version: 3 },
		};
		let why = refused(hang::Catalog::default_track_info(), &newer, stamp(1)).await;
		assert!(why.contains("composition version 3 is not supported"), "{why}");

		let mut untimed = hang::Catalog::<composition::Ext>::default();
		untimed.ext.composition = Some(composition(Duration::ZERO));
		let why = refused(Default::default(), &untimed, moq_net::Timestamp::unscaled(0)).await;
		assert!(why.contains("not timed"), "{why}");
	}

	/// A guest's broadcast, published independently, reaches a viewer through
	/// a relay as the studio played it: the composition lists it where it is,
	/// at the offset its timeline sits from the program's where the studio
	/// shows it, never sooner after its stamp than its catalog's clock and the
	/// studio's playout delay allow, and a player of that composition reads
	/// the guest from the relay on the program's clock at that offset.
	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "needs a Vulkan device and an H.264 encoder; run by just render-sw"]
	async fn plays_a_guest_where_the_studio_did() {
		use moq_mux::catalog::Stream;
		const TIMEOUT: Duration = Duration::from_secs(30);
		const PERIOD: Duration = Duration::from_millis(33);

		let _ = moq_tokio::crypto::install_default();
		let fixture = moq_relay::test_relay().await.expect("test relay");
		let ready = fixture.relay.ready();
		let relay = fixture.relay.cluster().origin.clone();
		tokio::spawn(fixture.relay.run());
		ready.wait().await.expect("relay ready");

		// The guest, on a clock of its own started well before the program's.
		let guest = moq_mux::Clock::new();
		kio::clock::sleep(Duration::from_millis(1_500)).await;
		let publication = relay
			.route("guests/alice", moq_net::origin::RouteOptions::default())
			.unwrap();
		let mut broadcast = publication.broadcast().clone();
		let config = moq_mux::catalog::Config::default().with_clock(guest.clone());
		let catalog = moq_mux::catalog::Producer::new(&mut broadcast, config).unwrap();
		// Its encoder stays on the thread that made it.
		let stamping = guest.clone();
		std::thread::spawn(move || {
			let runtime = tokio::runtime::Builder::new_current_thread()
				.enable_all()
				.build()
				.unwrap();
			runtime.block_on(async move {
				let mut config = moq_video::encode::Config::new(64, 64, moq_video::FrameRate::new(30, 1).unwrap());
				config.kind = moq_video::encode::Kind::Software;
				let rendition = config.probe().await.unwrap();
				let reserved = catalog.timebase().reserve().with_interval(config.effective_join());
				let track = broadcast
					.unique_track(".avc3", reserved.track_info(hang::catalog::PRIORITY.video))
					.unwrap();
				let mut video = moq_video::encode::Producer::new(track, reserved, rendition).unwrap();
				let mut encoder = moq_video::encode::Encoder::new(&config).unwrap();
				let gray = vec![0x80; 64 * 64 * 4];
				let mut pace = kio::clock::interval(PERIOD);
				loop {
					pace.tick().await;
					let surface = moq_video::Surface::rgba(&gray, moq_video::Size::new(64, 64)).unwrap();
					let frame = moq_video::Frame::new(surface, stamping.now());
					encoder.encode(&frame).unwrap();
					while let Some(encoded) = encoder.receive().unwrap() {
						if video.publish(&encoded).is_err() {
							return;
						}
					}
				}
			});
		});

		let mut client = moq_tokio::connect::Config::default();
		client.tls.fingerprint = vec![fixture.fingerprint.clone()];
		let client = client.init(Default::default()).expect("client");
		let dir = tempfile::tempdir().unwrap();
		let gpu = Gpu::new(wgpu::Backends::all()).await.expect("a GPU");
		let (autosave, _) = crate::autosave::Autosave::open(dir.path().join("show.toml")).unwrap();
		let mut studio = Collection::new();
		studio.canvas = Canvas {
			width: 640,
			height: 360,
			framerate: 30,
		};
		studio.sources.insert(
			"bg".into(),
			Source::Color {
				color: moq_scene::Rgba([0, 0, 255, 255]),
			},
		);
		studio.sources.insert(
			"alice".into(),
			Source::Broadcast {
				path: "guests/alice".into(),
			},
		);
		studio.scenes[0].items = vec![
			Item::new("bg", Rect::fill(&studio.canvas)),
			Item::new(
				"alice",
				Rect {
					x: 16.0,
					y: 16.0,
					width: 160.0,
					height: 90.0,
				},
			),
		];
		studio.published.insert("alice".into());
		studio.validate().unwrap();
		let launch = engine::Launch {
			gpu,
			recordings: dir.path().to_path_buf(),
			sessions: Default::default(),
		};
		let loaded = crate::autosave::Loaded {
			collection: studio,
			history: moq_scene::History::new(),
			dropped: 0,
		};
		let engine = engine::Engine::start(launch, autosave, loaded)
			.await
			.expect("the studio starts");
		let relays = vec![engine::Relay {
			client,
			url: fixture.url.clone(),
		}];
		// It reads the guest through the relay, and goes live on it.
		engine.watch(&relays);
		let destination = engine::Destination {
			relays,
			broadcast: "studio".into(),
			on_demand: false,
		};
		engine.go_live(destination).await.expect("the studio goes live");

		// The composition, as a viewer on the relay reads it.
		let viewer = relay.consume();
		let follow = |path: &'static str| {
			let viewer = viewer.clone();
			async move {
				kio::clock::timeout(TIMEOUT, viewer.follow(path, None).unwrap().available())
					.await
					.expect("the relay routes it")
					.unwrap()
					.expect("an instance")
			}
		};
		let program = follow("studio").await;
		let mut catalogs = moq_mux::catalog::Consumer::<()>::new(&program, Default::default())
			.await
			.unwrap();
		let program_clock = catalogs.next().await.unwrap().expect("a catalog").sync.clock.unwrap();
		let composed = follow("studio/composition").await;
		let track = composed.track(hang::Catalog::DEFAULT_NAME).unwrap();
		let mut compositions = hang::catalog::Consumer::<composition::Ext>::new(track, MAX_DELAY);
		// Where the two clocks put one moment, and so the offset with no delay.
		let micros = |at: Duration| at.as_micros() as i64;
		let wall = |clock: hang::catalog::Clock| micros(Duration::try_from(clock.wall).unwrap());
		let apart = wall(guest.wall().unwrap()) - wall(program_clock);
		let near = micros(composition::DRIFT) + micros(PERIOD);
		// Where the studio shows the guest now: the program's moment less the
		// guest's, which trails its stamp by the playout delay and the path's.
		let shown = || {
			let state = engine.state.lock().unwrap();
			let feed = state.sources.get("alice")?;
			let playout = feed.delay()?;
			let showing = feed.clock()?.position(kio::clock::now())?;
			// The program's moment now, by the guest's clock and where the two sit.
			let program = micros(Duration::try_from(guest.now()).ok()?) + apart;
			Some((program - micros(showing), micros(playout)))
		};
		// The studio's schedule for the guest settles as its first frames
		// arrive, a group's worth at once, and the composition follows it.
		let mut last = None;
		let settled = kio::clock::timeout(TIMEOUT, async {
			loop {
				let read = compositions.next().await.unwrap().expect("a catalog");
				let Some(composition) = read.value.ext.composition else {
					continue;
				};
				let Some(alice) = composition.sources.get("alice") else {
					continue;
				};
				let Some((offset, playout)) = shown() else { continue };
				tracing::info!(listed = alice.offset, offset, apart, playout, "the guest's offset");
				last = Some((alice.offset, offset));
				if (offset - near..=offset + near).contains(&alice.offset) {
					// Never sooner after its stamp than the playout delay.
					assert!(alice.offset - apart >= playout - near, "ahead of the playout delay");
					return composition;
				}
			}
		})
		.await;
		let Ok(composition) = settled else {
			panic!("the composition listed the guest at, and the studio showed it at, {last:?} us");
		};
		let alice = &composition.sources["alice"];
		assert_eq!(alice.broadcast.as_str(), "../guests/alice");
		assert!(alice.instance.as_ref().is_none_or(|instance| !instance.is_minted()));
		assert_eq!(composition.pictures.len(), 1);

		// A player of the composition, on the relay.
		let gpu = Gpu::new(wgpu::Backends::all()).await.expect("a GPU");
		let (bus, driver) = moq_audio::bus::Bus::new(Default::default()).unwrap();
		let (_mixer, mixed) = source::Mixer::new(driver, Instant::now());
		let mut sources = Sources::new(source::Context {
			bus,
			mixed,
			origin: viewer.clone(),
			dir: dir.path().to_path_buf(),
			output: moq_video::render::output(&gpu.device),
			publish: None,
			grants: None,
			browser: None,
		});
		let paths = Paths {
			program: moq_net::Path::new("studio").unwrap(),
			at: moq_net::Path::new("studio/composition").unwrap(),
		};
		let played = collection(Some(&composition), CANVAS, &paths).unwrap();
		for driver in sources.sync(&played) {
			engine::spawn(driver);
		}
		assert_eq!(
			played.sources["layer/alice"],
			Source::Layer {
				clock: PROGRAM.into(),
				path: "guests/alice".into(),
				instance: alice.instance.clone(),
				offset: alice.offset,
			}
		);
		kio::clock::timeout(TIMEOUT, async {
			while sources.get("layer/alice").unwrap().status() != Status::Live {
				kio::clock::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("the player reads the guest from the relay");

		let stopping = engine.clone();
		tokio::task::spawn_blocking(move || stopping.stop()).await.unwrap();
		kio::clock::timeout(TIMEOUT, engine.close())
			.await
			.expect("the studio closes");
	}
}
