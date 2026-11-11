//! `--watch`: play a studio broadcast as a viewer would, the program with each
//! source the [layer layout](moq_scene::layout) names composited over it and
//! mixed with it, on the program's media clock.
//!
//! A prototype for settling the layout's format before hang specifies one,
//! built on moq-scene: a collection the layout drives, rather than a scene
//! file, whose one scene draws the program, a [`Source::Broadcast`], under
//! each picture's [`Source::Layer`]. Each layer plays its own tracks on the
//! program's playout clock, each layout takes effect when the program reaches
//! the media time it is stamped with, and its pictures' keyframes play from
//! its `start` on that clock. A layout of another version is refused loudly,
//! and the program plays alone.

use std::collections::{BTreeSet, VecDeque};
use std::time::Duration;

use anyhow::Context;
use kio::clock::Instant;
use moq_scene::layout::{self, Layout};
use moq_scene::publish::Ext;
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

/// The canvas until the program's catalog or a layout gives its size.
const CANVAS: Canvas = Canvas {
	width: 1280,
	height: 720,
	framerate: 30,
};

/// Play the broadcast at `url` until the window closes. Its path names the
/// broadcast, as `moq play`'s does, and `client` dials it.
pub fn run(client: moq_tokio::connect::Config, quic: moq_tokio::quic::Config, url: url::Url) -> anyhow::Result<()> {
	let runtime = tokio::runtime::Runtime::new()?;
	let _runtime = runtime.enter();
	let client = client.init(quic)?;
	let origin = moq_tokio::origin::spawn();
	tracing::info!(url = %moq_tokio::RedactedUrl::new(&url), "watching");
	let connection = client.with_subscriber(origin.clone()).connect(url.clone());

	let gpu = pollster::block_on(Gpu::new())?;
	let (bus, mixer) = moq_audio::bus::Bus::new(Default::default())?;
	let config = bus.config().clone();
	let start = Instant::now();
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
		origin: origin.consume(),
		dir: std::env::current_dir()?,
		output: moq_video::render::output(&gpu.device),
		publish: None,
		browser: None,
	});
	let collection = collection(None, CANVAS)?;
	let mut compositor = Compositor::new(&gpu.device, &gpu.queue, collection.canvas)?;
	compositor.set_previewing(false);
	compositor.set_preview(Some(SCENE.into()));
	compositor.cut(&collection, &sources)?;

	let (read, _) = watch::channel(Read::default());
	runtime.spawn(follow(origin.consume(), read.clone()));

	let watch = Watch {
		url: moq_tokio::RedactedUrl::new(&url).to_string(),
		read,
		collection,
		synced: false,
		layout: None,
		start: None,
		size: None,
		sources,
		compositor,
		monitor,
		shown: None,
	};
	let options = eframe::NativeOptions {
		viewport: egui::ViewportBuilder::default()
			.with_title("MoQ Studio")
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
async fn advance(start: Instant, config: moq_audio::bus::Config, mut mixer: moq_audio::bus::Driver) {
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

/// What the broadcast's catalog and layout track say, for the window to apply.
#[derive(Default)]
struct Read {
	/// The program's picture size, from its catalog.
	size: Option<(u32, u32)>,
	/// Each layout read, or `None` for the program alone, and the media time
	/// it takes effect at, or `None` for at once, oldest first.
	pending: VecDeque<(Option<Layout>, Option<moq_net::Timestamp>)>,
	/// Why the layout was refused, so the program plays alone.
	refused: Option<String>,
}

impl Read {
	/// Queue `layout` to take effect at `at`. The program plays at most
	/// [`MAX_DELAY`] behind what arrives, so of the layouts stamped that far
	/// before it, all due by now, only the newest is kept.
	fn push(&mut self, layout: Option<Layout>, at: Option<moq_net::Timestamp>) {
		let due = |stamp: Option<moq_net::Timestamp>| match (stamp, at) {
			(None, _) => true,
			(Some(_), None) => true,
			(Some(stamp), Some(at)) => Duration::from(stamp) + MAX_DELAY <= Duration::from(at),
		};
		let passed = self.pending.iter().take_while(|(_, stamp)| due(*stamp)).count();
		self.pending.drain(..passed.saturating_sub(1));
		self.pending.push_back((layout, at));
	}
}

/// Read each instance of the root broadcast as it comes online: the program's
/// size from its catalog, and each layout from the layout track while the
/// catalog lists it.
async fn follow(origin: moq_net::origin::Consumer, read: watch::Sender<Read>) {
	let mut instances = match origin.follow("", None) {
		Ok(instances) => instances,
		Err(err) => {
			tracing::error!(%err, "cannot follow the broadcast");
			return;
		}
	};
	let mut playing = None;
	loop {
		tokio::select! {
			event = instances.available() => match event {
				Ok(Some(broadcast)) => playing = Some(Box::pin(instance(broadcast, read.clone()))),
				Ok(None) => playing = None,
				Err(err) => {
					tracing::error!(%err, "the broadcast's announcements failed");
					return;
				}
			},
			result = async { playing.as_mut().expect("guarded").await }, if playing.is_some() => {
				playing = None;
				if let Err(err) = result {
					tracing::warn!(error = format!("{err:#}"), "cannot read the broadcast's layout");
				}
			}
		}
	}
}

/// Read one instance of the broadcast until its catalog ends.
async fn instance(broadcast: moq_net::broadcast::Consumer, read: watch::Sender<Read>) -> anyhow::Result<()> {
	let mut catalogs =
		moq_mux::catalog::Consumer::<Ext>::new(&broadcast, moq_mux::catalog::CatalogFormat::default()).await?;
	let mut layouts: Option<moq_json::snapshot::Consumer<Layout>> = None;
	// A refused layout stays refused for this instance.
	let mut refused = false;
	let alone = || read.send_modify(|read| read.push(None, None));
	loop {
		tokio::select! {
			catalog = moq_mux::catalog::Stream::next(&mut catalogs) => {
				let Some(catalog) = catalog? else {
					alone();
					return Ok(());
				};
				let size = catalog.video.renditions.values().find_map(|config| config.coded_width.zip(config.coded_height));
				read.send_if_modified(|read| std::mem::replace(&mut read.size, size) != size);
				let listed = catalog.json.tracks.contains_key(layout::TRACK);
				if listed && layouts.is_none() && !refused {
					let track = broadcast.track(layout::TRACK)?;
					layouts = Some(moq_json::snapshot::Consumer::new(track, Default::default()));
				} else if !listed && layouts.take().is_some() {
					alone();
				}
			}
			next = async { layouts.as_mut().expect("guarded").next().await }, if layouts.is_some() => match next {
				Ok(Some(timed)) => read.send_modify(|read| read.push(Some(timed.value), timed.at)),
				Ok(None) => {
					layouts = None;
					alone();
				}
				Err(err) => {
					tracing::error!(%err, "refusing the layer layout; playing the program alone");
					layouts = None;
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

/// The collection `layout` drives on a canvas the program's `size` when it
/// has none: the program, and over it each picture's layer, with the program
/// and each sound's layer in the audio scene on program.
fn collection(layout: Option<&Layout>, size: Canvas) -> anyhow::Result<Collection> {
	let mut collection = Collection::new();
	collection.canvas = match layout {
		Some(layout) => Canvas {
			width: layout.canvas.width,
			height: layout.canvas.height,
			framerate: size.framerate,
		},
		None => size,
	};
	collection
		.sources
		.insert(PROGRAM.into(), Source::Broadcast { path: String::new() });
	let mut items = vec![Item::new(PROGRAM, Rect::fill(&collection.canvas))];
	let mut sounds = BTreeSet::from([PROGRAM.to_string()]);
	let mut layer = |source: &str| {
		let name = format!("{LAYER}{source}");
		collection.sources.insert(
			name.clone(),
			Source::Layer {
				broadcast: PROGRAM.into(),
				source: source.into(),
			},
		);
		name
	};
	let mut audio = Vec::new();
	for picture in layout.iter().flat_map(|layout| &layout.pictures) {
		items.push(Item {
			source: layer(&picture.item.source),
			..picture.item.clone()
		});
	}
	for sound in layout.iter().flat_map(|layout| &layout.sounds) {
		let name = layer(&sound.source);
		audio.push((name.clone(), sound.audio.clone()));
		sounds.insert(name);
	}
	collection.audio.extend(audio);
	collection.scenes = vec![Scene {
		items,
		..Scene::new(SCENE)
	}];
	collection.audio_scenes = vec![AudioScene {
		program: true,
		sounds,
		..AudioScene::new(AudioScene::DEFAULT)
	}];
	collection.validate().context("cannot play the layer layout")?;
	Ok(collection)
}

/// The player: the collection the layout drives, and what plays it.
struct Watch {
	/// What the window says it waits for.
	url: String,
	read: watch::Sender<Read>,
	collection: Collection,
	/// Whether the sources run what `collection` holds.
	synced: bool,
	/// The layout in effect, or `None` for the program alone.
	layout: Option<Layout>,
	/// When the program scene's keyframes play from, as the layout in effect
	/// set it.
	start: Option<Duration>,
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
		let schedule = self.sources.get(PROGRAM).and_then(|feed| feed.clock()).cloned();
		let mut changed = !self.synced;
		self.read.send_if_modified(|read| {
			if read.size != self.size {
				self.size = read.size;
				changed = true;
			}
			// Each layout once the program plays the frame it takes effect on.
			while let Some((_, at)) = read.pending.front() {
				let due = match at {
					None => true,
					Some(at) => schedule
						.as_ref()
						.and_then(|schedule| schedule.due(*at))
						.is_some_and(|due| due <= now),
				};
				if !due {
					break;
				}
				let (layout, _) = read.pending.pop_front().expect("peeked");
				self.layout = layout;
				changed = true;
			}
			false
		});

		if changed {
			let size = match self.size {
				Some((width, height)) => Canvas {
					width,
					height,
					..CANVAS
				},
				None => CANVAS,
			};
			let collection = match collection(self.layout.as_ref(), size) {
				Ok(collection) => collection,
				Err(err) => {
					tracing::error!(error = format!("{err:#}"), "playing the program alone");
					self.read.send_modify(|read| read.refused = Some(format!("{err:#}")));
					self.layout = None;
					collection(None, size)?
				}
			};
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

		// A new start is a scene taken, whose keyframes play from it.
		let start = self.layout.as_ref().map(|layout| layout.start);
		if start != self.start
			&& let Some(start) = start
			&& let Ok(stamp) = moq_net::Timestamp::try_from(start)
			&& let Some(due) = schedule.as_ref().and_then(|schedule| schedule.due(stamp))
		{
			self.compositor.set_program_start(due);
			self.start = Some(start);
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
	use moq_scene::layout::{Picture, Sound, Version};
	use moq_scene::{Audio, Filter, Keyframe, Size};

	use super::*;

	/// A camera named as the program is, placed twice, and a microphone.
	fn layout(start: Duration) -> Layout {
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
		let picture = Picture {
			item: cam,
			priority: 0,
			toggle: false,
		};
		Layout {
			version: Version,
			canvas: Size {
				width: 640,
				height: 360,
			},
			start,
			pictures: vec![picture.clone(), picture],
			sounds: vec![Sound {
				source: "mic".into(),
				audio: Audio {
					gain: 0.5,
					delay: Duration::from_millis(80),
					..Default::default()
				},
				priority: 0,
				toggle: false,
			}],
		}
	}

	fn stamp(secs: u64) -> moq_net::Timestamp {
		moq_net::Timestamp::try_from(Duration::from_secs(secs)).unwrap()
	}

	/// The layout's pictures draw over the program from layers named apart
	/// from it, even one named as it is, a source placed twice shares one
	/// layer, and the audio scene on program plays the program and each
	/// sound at its gain and delay.
	#[test]
	fn a_layout_drives_the_collection() {
		let layout = layout(Duration::ZERO);
		let driven = collection(Some(&layout), CANVAS).unwrap();
		assert_eq!((driven.canvas.width, driven.canvas.height), (640, 360));
		let items = &driven.scene(SCENE).unwrap().items;
		let sources: Vec<&str> = items.iter().map(|item| item.source.as_str()).collect();
		assert_eq!(sources, [PROGRAM, "layer/program", "layer/program"]);
		assert_eq!(items[1].keyframes, layout.pictures[0].item.keyframes);
		assert_eq!(
			driven.sources["layer/program"],
			Source::Layer {
				broadcast: PROGRAM.into(),
				source: "program".into()
			}
		);
		let heard = driven.heard(AudioScene::DEFAULT);
		assert_eq!(heard, BTreeSet::from([PROGRAM, "layer/mic"]));
		assert_eq!(driven.audio["layer/mic"], layout.sounds[0].audio);

		let alone = collection(None, CANVAS).unwrap();
		assert_eq!(alone.sources.len(), 1, "the program alone");
		assert_eq!(alone.canvas, CANVAS);
	}

	/// Of the layouts the program must have reached by the time a new one
	/// arrives, only the newest waits.
	#[test]
	fn only_the_newest_passed_layout_waits() {
		let mut read = Read::default();
		for secs in [1, 2, 3, 4] {
			read.push(None, Some(stamp(secs)));
		}
		read.push(None, Some(stamp(5)));
		let stamps: Vec<_> = read.pending.iter().map(|(_, at)| *at).collect();
		assert_eq!(stamps, [Some(stamp(3)), Some(stamp(4)), Some(stamp(5))]);
	}

	/// A layout of another version, as a newer studio would publish.
	#[derive(serde::Serialize)]
	struct Newer {
		version: u32,
	}

	/// A broadcast with a catalog, and the instance reader's view of it.
	fn broadcast() -> (
		moq_net::broadcast::Producer,
		moq_scene::publish::Catalog,
		watch::Sender<Read>,
		tokio::task::JoinHandle<anyhow::Result<()>>,
	) {
		let mut broadcast = moq_net::broadcast::Info::new().produce();
		let config = moq_mux::catalog::Config::default().with_catalog(Default::default());
		let catalog = moq_scene::publish::Catalog::new(&mut broadcast, config).unwrap();
		let (read, _) = watch::channel(Read::default());
		let reader = tokio::spawn(instance(broadcast.consume(), read.clone()));
		(broadcast, catalog, read, reader)
	}

	/// What `read` holds once it has `count` layouts waiting, with their
	/// stamps as durations, since one read back carries its track's timescale.
	async fn pending(read: &watch::Sender<Read>, count: usize) -> Vec<(Option<Layout>, Option<Duration>)> {
		let mut reads = read.subscribe();
		let read = reads.wait_for(|read| read.pending.len() >= count).await.unwrap();
		let pending = read.pending.iter().cloned();
		pending.map(|(layout, at)| (layout, at.map(Duration::from))).collect()
	}

	/// The reader hands over each layout at the media time it took effect,
	/// and the program alone once the catalog stops listing the track, until
	/// it lists it again.
	#[tokio::test]
	async fn reads_each_layout() {
		let (broadcast, catalog, read, reader) = broadcast();
		let mut publisher = layout::Publisher::new(broadcast.clone(), catalog.clone());
		publisher.update(layout(Duration::ZERO), stamp(1)).unwrap();
		let first = (Some(layout(Duration::ZERO)), Some(Duration::from_secs(1)));
		assert_eq!(pending(&read, 1).await, [first]);
		publisher.finish().unwrap();
		assert_eq!(
			pending(&read, 2).await[1],
			(None, None),
			"the program alone once unlisted"
		);
		publisher.update(layout(Duration::from_secs(2)), stamp(2)).unwrap();
		let again = (Some(layout(Duration::from_secs(2))), Some(Duration::from_secs(2)));
		assert_eq!(pending(&read, 3).await[2], again, "listed again");
		reader.abort();
	}

	/// A layout of another version is refused, saying why, and the program
	/// plays alone.
	#[tokio::test]
	async fn refuses_another_version() {
		let (broadcast, catalog, read, reader) = broadcast();
		let info = moq_net::track::Info::default().with_timescale(moq_mux::Clock::TIMESCALE);
		let track = broadcast.create_track(layout::TRACK, info).unwrap();
		let options = moq_mux::json::Options::default().with_deltas(false);
		let mut newer = catalog
			.json_snapshot::<Newer>(track, moq_mux::json::Config::default(), options)
			.unwrap();
		newer.update(&Newer { version: 2 }, stamp(1)).unwrap();
		assert_eq!(pending(&read, 1).await, [(None, None)]);
		let refused = read.borrow().refused.clone().unwrap();
		assert!(refused.contains("layer layout version 2 is not supported"), "{refused}");
		reader.abort();
	}
}
