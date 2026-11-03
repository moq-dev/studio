//! Everything behind the window: the GPU, the sources, the program clock that
//! draws the canvas and mixes the audio, and the broadcast it publishes: the
//! program, and each local source's own track beside it.
//!
//! Threads:
//! - **program** draws preview and program once per canvas frame and hands the
//!   program to each rung's encoder its gate lets encode, scaled to the rung
//!   and stamped with the program clock.
//! - **audio** reads the program bus on the same clock and publishes it.
//! - **encode-***: one per rung of the program's ladder, which waits for each
//!   program copy and encodes it.
//! - **source-***: one per running source, since a capture stream may not
//!   leave the thread that opened it.
//! - **autosave** writes the collection and its history to their files once
//!   edits to them settle.
//! - The UI runs on the main thread and edits [`State`] under its lock.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::Context;
use bytes::Bytes;
use kio::clock::Instant;
use moq_scene::ladder::{self, Gate, Rendition, Step};
use moq_scene::source::{self, Driver, Sources};
use moq_scene::{Collection, Compositor, Draft, History, Scaler, wgpu};
use moq_video::render::export::{Exporter, Pending, Route};

use crate::autosave::{self, Autosave};
use crate::health;

/// The GPU everything shares: sources upload to it, the compositor draws on
/// it, the window presents from it, and the encoder reads the program from it.
#[derive(Clone)]
pub struct Gpu {
	pub instance: wgpu::Instance,
	pub adapter: wgpu::Adapter,
	pub device: wgpu::Device,
	pub queue: wgpu::Queue,
}

impl Gpu {
	pub async fn new() -> anyhow::Result<Self> {
		let instance = wgpu::Instance::default();
		let adapter = instance
			.request_adapter(&wgpu::RequestAdapterOptions {
				power_preference: wgpu::PowerPreference::HighPerformance,
				..Default::default()
			})
			.await
			.context("no GPU adapter")?;
		// Lets a hardware-decoded or captured frame reach the compositor, and the
		// program reach the encoder, without a copy where the driver allows.
		let descriptor = wgpu::DeviceDescriptor {
			label: Some("moq-studio"),
			..Default::default()
		};
		let (device, queue) = moq_video::render::request_device(&adapter, &descriptor)
			.await
			.context("cannot open the GPU")?;
		Ok(Self {
			instance,
			adapter,
			device,
			queue,
		})
	}
}

/// What the UI edits and the program clock draws.
pub struct State {
	/// Changed only through [`commit`](Self::commit) and the history, so every
	/// edit can be undone.
	collection: Collection,
	history: History,
	/// History entries the last load dropped, since they no longer applied to
	/// the scene file.
	pub dropped: usize,
	pub sources: Sources,
	pub compositor: Compositor,
	/// Edited since the sources were last synced.
	stale: bool,
	/// For the autosave, which writes the collection once edits settle.
	edits: autosave::Edits,
}

impl State {
	pub fn collection(&self) -> &Collection {
		&self.collection
	}

	pub fn history(&self) -> &History {
		&self.history
	}

	/// Apply `draft` and record it in the history; the sources follow it on
	/// the next frame, and the files shortly after. `gesture` names the
	/// interaction that made it, so the steps of one drag undo together.
	pub fn commit(&mut self, draft: Draft, gesture: Option<u64>) -> anyhow::Result<()> {
		let edit = draft.finish()?;
		self.collection.apply(&edit)?;
		self.history.record(edit, gesture);
		self.edited();
		Ok(())
	}

	/// Undo the newest edit, returning its label, or `None` with none left.
	pub fn undo(&mut self) -> anyhow::Result<Option<String>> {
		let label = self.history.undo(&mut self.collection)?.map(|edit| edit.label.clone());
		if label.is_some() {
			self.edited();
		}
		Ok(label)
	}

	/// Redo the oldest undone edit, returning its label, or `None` with none left.
	pub fn redo(&mut self) -> anyhow::Result<Option<String>> {
		let label = self.history.redo(&mut self.collection)?.map(|edit| edit.label.clone());
		if label.is_some() {
			self.edited();
		}
		Ok(label)
	}

	/// Undo or redo to just after the history's `position`th entry.
	pub fn seek(&mut self, position: usize) -> anyhow::Result<()> {
		self.history.seek(&mut self.collection, position)?;
		self.edited();
		Ok(())
	}

	/// After any change to the collection or its history, which the autosave
	/// writes once they settle.
	fn edited(&mut self) {
		self.stale = true;
		self.edits.record();
	}

	/// Swap in a collection and history reloaded from their files.
	fn replace(&mut self, loaded: autosave::Loaded) -> anyhow::Result<()> {
		let autosave::Loaded {
			collection,
			history,
			dropped,
		} = loaded;
		anyhow::ensure!(
			collection.canvas == self.collection.canvas,
			"the canvas changed; restart the studio to use {}x{} at {} fps",
			collection.canvas.width,
			collection.canvas.height,
			collection.canvas.framerate
		);
		// A scene in preview or on program that the file no longer has draws
		// black until another is picked.
		self.collection = collection;
		self.history = history;
		self.dropped = dropped;
		self.stale = true;
		Ok(())
	}
}

/// How the output is doing, for the stream panel.
#[derive(Default)]
pub struct Stats {
	/// Canvas frames drawn.
	pub rendered: AtomicU64,
	/// Program frames offered to an encoder, one per rung encoding it.
	pub offered: AtomicU64,
	/// Program frames handed to an encoder.
	pub copied: AtomicU64,
	/// Microseconds the last one waited on the GPU, and on a readback converted
	/// pixels.
	pub copy_us: AtomicU64,
	/// Whether the encoder reads the program on the GPU, rather than each frame
	/// being read back to the CPU.
	pub zero_copy: AtomicBool,
	/// Program frames read back to the CPU for the encoder.
	pub read_back: AtomicU64,
	/// Program frames not encoded because the encoder still held every copy.
	pub dropped: AtomicU64,
	/// Program frames encoded, over every rung.
	pub encoded: AtomicU64,
	/// Microseconds the last encode took.
	pub encode_us: AtomicU64,
	/// Whether any viewer is subscribed to any rung of the video.
	pub watched: AtomicBool,
	/// The encoder backend, once the largest rung's opened.
	pub encoder: Mutex<Option<String>>,
	/// Why the output stopped, if it did.
	pub error: Mutex<Option<String>>,
}

impl Stats {
	fn fail(&self, err: impl std::fmt::Display) {
		tracing::error!(%err, "output stopped");
		*self.error.lock().unwrap() = Some(err.to_string());
	}
}

/// Where the program goes.
pub struct Output {
	/// The broadcast path on the relay.
	pub path: String,
	/// Whether the program video's largest rung is copied and encoded only
	/// while someone watches, rather than all the time. The smaller rungs
	/// always are.
	pub on_demand: bool,
	/// The relay connection, which reconnects on its own and dials only while
	/// held. The Health panel reads its state.
	_connection: moq_tokio::Connection,
	publication: moq_net::origin::Publication,
	/// The audio's claim on the connection's bandwidth, which it does not
	/// follow: Opus at this rate is never what congests the uplink.
	_audio: moq_net::bandwidth::Reservation,
}

impl Drop for Output {
	fn drop(&mut self) {
		self.publication.close();
	}
}

/// The running studio.
pub struct Engine {
	pub state: Mutex<State>,
	pub autosave: Autosave,
	pub stats: Stats,
	/// How the stream is delivered, graded every [`health::INTERVAL`].
	pub health: Mutex<health::Report>,
	pub bus: moq_audio::bus::Bus,
	/// `None` when running without a relay.
	pub output: Option<Output>,
	/// Keeps broadcast sources' origin open when no connection holds it.
	_origin: Option<moq_net::origin::Producer>,
}

/// The audio the program publishes.
const AUDIO_BITRATE: moq_net::bandwidth::Rate = moq_net::bandwidth::Rate::from_kbps(128);

/// How often the audio thread reads the bus. Short enough that a read is never
/// close to the bus's buffering, long enough to cost nothing.
const AUDIO_PERIOD: Duration = Duration::from_millis(10);

impl Engine {
	/// Open the `loaded` collection and history on `gpu`, start the program
	/// clock, and save both to `autosave`'s files as they change.
	/// With `connect`, publish the program at `broadcast` on that relay,
	/// encoding its video only while someone watches when `on_demand`.
	///
	/// Call inside the tokio runtime that serves the relay connection.
	pub async fn start(
		gpu: Gpu,
		autosave: Autosave,
		loaded: autosave::Loaded,
		connect: Option<(moq_tokio::Client, url::Url)>,
		broadcast: &str,
		on_demand: bool,
	) -> anyhow::Result<Arc<Self>> {
		let bus = moq_audio::bus::Bus::new(Default::default())?;
		let subscribe = moq_tokio::origin::spawn();
		let sources = subscribe.consume();
		let autosave::Loaded {
			collection,
			history,
			dropped,
		} = loaded;
		let canvas = collection.canvas;

		let (output, origin, publish) = match connect {
			Some((client, url)) => {
				let (output, publish) = publish(&gpu, client, url, subscribe, broadcast, canvas, on_demand).await?;
				(Some(output), None, Some(publish))
			}
			None => (None, Some(subscribe), None),
		};

		// Absolute, so a copied image names the same file in another collection.
		let dir = autosave.path().parent().filter(|dir| !dir.as_os_str().is_empty());
		let dir = std::path::absolute(dir.unwrap_or(Path::new(".")))?;
		let sources = Sources::new(source::Context {
			bus: bus.clone(),
			origin: sources,
			dir,
			output: moq_video::render::output(&gpu.device),
			publish: publish.as_ref().map(|publish| publish.sources.clone()),
		});
		let mut compositor = Compositor::new(&gpu.device, &gpu.queue, collection.canvas)?;
		compositor.set_preview(collection.scenes.first().map(|scene| scene.name.clone()));
		compositor.cut();

		let engine = Arc::new(Self {
			state: Mutex::new(State {
				collection,
				history,
				dropped,
				sources,
				compositor,
				stale: true,
				edits: autosave.edits(),
			}),
			autosave,
			stats: Stats::default(),
			health: Mutex::default(),
			bus,
			output,
			_origin: origin,
		});

		let (video, audio, clock, publisher) = match publish {
			Some(Publish {
				rungs,
				audio,
				clock,
				health,
				sources: _,
			}) => {
				let zero_copy = rungs[0].exporter.route() == Route::Gpu;
				engine.stats.zero_copy.store(zero_copy, Ordering::Relaxed);
				let program = engine.state.lock().unwrap().compositor.program().clone();
				let mut video = Vec::with_capacity(rungs.len());
				for (index, rung) in rungs.into_iter().enumerate() {
					let Rung {
						gate,
						exporter,
						producer,
						config,
					} = rung;
					// The largest rung is the canvas itself; the others draw it smaller.
					let scaler = (index > 0).then(|| Scaler::new(&gpu.device, &gpu.queue, &program, exporter.size()));
					// One more than the exporter's pool, for a step behind a full pool.
					let (sender, pending) = mpsc::sync_channel(Exporter::DEPTH + 1);
					let encoder = engine.clone();
					let size = exporter.size();
					// The largest rung holds its encoder from the start. Opened before
					// the program clock starts the sources, so the program holds its
					// encode session even while unwatched, and a source past the GPU's
					// session cap is the one that fails.
					let (opened, open) = tokio::sync::oneshot::channel();
					let opened = (index == 0).then_some(opened);
					std::thread::Builder::new()
						.name(format!("encode-{}p", size.height))
						.spawn(move || encode(&encoder, opened, pending, producer, config, zero_copy))?;
					if index == 0 {
						open.await.context("the encode thread stopped")??;
					}
					video.push(Feeder {
						gate,
						scaler,
						exporter,
						sender,
						step: None,
					});
				}
				(Some(video), Some(audio), Some(clock), Some(health))
			}
			None => (None, None, None, None),
		};

		let start = Instant::now();
		// Where the program clock starts on the catalog's, so the wall time the
		// catalog advertises for a frame is when the program drew it.
		let epoch = match clock {
			Some(clock) => Duration::from(clock.capture(start)?),
			None => Duration::ZERO,
		};
		let program = engine.clone();
		std::thread::Builder::new()
			.name("program".into())
			.spawn(move || clocked(program.run_program(start, epoch, video.unwrap_or_default())))?;
		let mix = engine.clone();
		std::thread::Builder::new()
			.name("audio".into())
			.spawn(move || clocked(mix.run_audio(start, epoch, audio)))?;
		let save = engine.clone();
		std::thread::Builder::new()
			.name("autosave".into())
			.spawn(move || clocked(save.autosave.run(|| save.serialize())))?;
		tokio::spawn(engine.clone().grade(health::Sampler::new(publisher)));

		Ok(engine)
	}

	/// Write the collection and its history to their files unless they
	/// already hold them.
	pub fn save(&self) -> anyhow::Result<()> {
		self.autosave.save(|| self.serialize())
	}

	/// Both under one lock, so the history describes the collection beside it.
	fn serialize(&self) -> anyhow::Result<autosave::Text> {
		let state = self.state.lock().unwrap();
		autosave::Text::new(&state.collection, &state.history)
	}

	/// Replace the collection and its history with what the files hold now.
	pub fn reload(&self) -> anyhow::Result<()> {
		self.autosave
			.reload(|loaded| self.state.lock().unwrap().replace(loaded))
	}

	/// Grade the stream's health every interval, for the Health panel.
	async fn grade(self: Arc<Self>, mut sampler: health::Sampler) {
		let mut interval = kio::clock::interval(health::INTERVAL);
		interval.set_missed_tick_behavior(kio::clock::MissedTickBehavior::Skip);
		loop {
			let now = interval.tick().await;
			let report = sampler.sample(now, self.local(now));
			*self.health.lock().unwrap() = report;
		}
	}

	/// What only the studio knows about its stream at `now`.
	fn local(&self, now: Instant) -> health::Local {
		let stats = &self.stats;
		let watched = stats.watched.load(Ordering::Relaxed);
		let stalled = {
			let state = self.state.lock().unwrap();
			let program = state
				.compositor
				.program_scene()
				.and_then(|name| state.collection.scene(name));
			let mut shown: Vec<&str> = program
				.into_iter()
				.flat_map(|scene| &scene.items)
				.filter(|item| item.visible)
				.map(|item| item.source.as_str())
				.collect();
			shown.sort_unstable();
			shown.dedup();
			shown
				.into_iter()
				.filter_map(|name| {
					let since = state.sources.get(name)?.stalled(now)?;
					Some((name.to_string(), since))
				})
				.collect()
		};
		health::Local {
			offered: stats.offered.load(Ordering::Relaxed),
			dropped: stats.dropped.load(Ordering::Relaxed),
			encoded: stats.encoded.load(Ordering::Relaxed),
			watched,
			encoder: stats.encoder.lock().unwrap().clone(),
			failed: stats.error.lock().unwrap().clone(),
			stalled,
		}
	}

	/// Draw a canvas frame per tick of the program clock, and hand the program
	/// to each rung's encoder, stamped `epoch` past `start`, while that rung's
	/// gate lets it encode.
	///
	/// Drawing outlives the encoders: a failed rung stops copying the program,
	/// and the preview and program keep moving.
	async fn run_program(&self, start: Instant, epoch: Duration, mut rungs: Vec<Feeder>) {
		let canvas = self.state.lock().unwrap().collection().canvas;
		let period = Duration::from_secs(1) / canvas.framerate;
		let mut clock = kio::clock::interval_at(start, period);
		clock.set_missed_tick_behavior(kio::clock::MissedTickBehavior::Skip);

		loop {
			let tick = clock.tick().await;
			let program = {
				let mut guard = self.state.lock().unwrap();
				let state = &mut *guard;
				if std::mem::take(&mut state.stale) {
					for driver in state.sources.sync(&state.collection) {
						spawn(driver);
					}
				}
				if let Err(err) = state.compositor.render(&state.collection, &state.sources, tick) {
					tracing::error!(%err, "cannot draw the canvas");
					continue;
				}
				state.compositor.program().clone()
			};
			self.stats.rendered.fetch_add(1, Ordering::Relaxed);

			if rungs.is_empty() {
				continue;
			}
			let watched = rungs.iter().any(|rung| rung.gate.watched());
			self.stats.watched.store(watched, Ordering::Relaxed);
			let Ok(timestamp) = moq_net::Timestamp::try_from(epoch + tick.saturating_duration_since(start)) else {
				continue;
			};
			// A rung whose encoder stopped, having reported why, is dropped.
			rungs.retain_mut(|rung| rung.feed(&self.stats, &program, timestamp));
		}
	}
	/// Read the program bus as the program clock advances, and publish it
	/// stamped `epoch` past `start`.
	///
	/// Runs without a relay too, so the meters move and sources never back up.
	async fn run_audio(&self, start: Instant, epoch: Duration, mut producer: Option<moq_audio::encode::Producer>) {
		let config = self.bus.config().clone();
		let rate = u128::from(config.sample_rate);
		let channels = config.layout.channels() as usize;
		let mut clock = kio::clock::interval_at(start, AUDIO_PERIOD);
		clock.set_missed_tick_behavior(kio::clock::MissedTickBehavior::Skip);
		let mut samples = Vec::new();
		let mut written: u128 = 0;

		loop {
			let tick = clock.tick().await;
			// Sample-exact against the program clock, so audio and video stay
			// aligned however the ticks jitter.
			let due = tick.saturating_duration_since(start).as_nanos() * rate / 1_000_000_000;
			let frames = (due - written) as usize;
			if frames == 0 {
				continue;
			}
			samples.resize(frames * channels, 0.0);
			self.bus.read(&mut samples);
			let timestamp = moq_net::Timestamp::from_micros((epoch.as_micros() + written * 1_000_000 / rate) as u64);
			written = due;

			let Some(publish) = producer.as_mut() else { continue };
			let Ok(timestamp) = timestamp else { continue };
			let data: Vec<u8> = samples.iter().flat_map(|sample| sample.to_le_bytes()).collect();
			if let Err(err) = publish.write(&moq_audio::Frame::new(Bytes::from(data), timestamp)) {
				self.stats.fail(format!("audio: {err}"));
				// Viewers see the audio fail rather than go away unfinished.
				if let Some(producer) = producer.take() {
					producer.abort(moq_net::StreamError::Internal.into());
				}
			}
		}
	}
}

/// The encoders behind an [`Output`], handed to their threads.
struct Publish {
	/// The program's ladder, largest first.
	rungs: Vec<Rung>,
	audio: moq_audio::encode::Producer,
	/// The catalog's clock, which the codec producers write timestamps
	/// against verbatim.
	clock: moq_mux::Clock,
	/// What the health sampler grades.
	health: health::Publisher,
	/// Where each local source publishes its own track.
	sources: moq_scene::publish::Publish,
}

/// One rung of the program's ladder, before its threads start.
struct Rung {
	gate: Gate,
	/// Copies the rung's size of the program for its encoder.
	exporter: Exporter,
	producer: moq_video::encode::Producer,
	config: moq_video::encode::Config,
}

/// One rung of the program's ladder, as the program clock feeds it.
struct Feeder {
	gate: Gate,
	/// Draws the program at the rung's size; `None` for the largest rung,
	/// which is the canvas.
	scaler: Option<Scaler>,
	exporter: Exporter,
	sender: mpsc::SyncSender<Video>,
	/// What the encoder was last told, so a rung skipping frames is told once.
	step: Option<Step>,
}

impl Feeder {
	/// Hand the encoder this tick's `program`, or tell it why not. Returns
	/// whether the encoder still runs.
	fn feed(&mut self, stats: &Stats, program: &wgpu::Texture, timestamp: moq_net::Timestamp) -> bool {
		let step = self.gate.step();
		let message = match step {
			Step::Encode(bitrate) => {
				stats.offered.fetch_add(1, Ordering::Relaxed);
				let texture = self.scaler.as_ref().map_or(program, Scaler::scale);
				match self.exporter.copy(texture, timestamp) {
					// The pool bounds the frames in flight, so this waits only
					// behind a step the encoder has not reached yet.
					Ok(Some(pending)) => Some(Video::Frame(pending, bitrate)),
					Ok(None) => {
						stats.dropped.fetch_add(1, Ordering::Relaxed);
						None
					}
					// The canvas and the exporter share a format, so this is the
					// GPU failing: the rung stops, and the canvas keeps drawing.
					Err(err) => {
						let _ = self.sender.send(Video::Failed(err));
						return false;
					}
				}
			}
			skip => (self.step != Some(skip)).then_some(Video::Step(skip)),
		};
		self.step = Some(step);
		// A send fails only once the encoder stopped, having reported why.
		message.is_none_or(|message| self.sender.send(message).is_ok())
	}
}

/// Announce the program broadcast, which also carries each local source's own
/// track, and connect to the relay, which also carries `subscribe`'s broadcast
/// sources.
async fn publish(
	gpu: &Gpu,
	client: moq_tokio::Client,
	url: url::Url,
	subscribe: moq_net::origin::Producer,
	path: &str,
	canvas: moq_scene::Canvas,
	on_demand: bool,
) -> anyhow::Result<(Output, Publish)> {
	let origin = moq_tokio::origin::spawn();
	let path = moq_net::Path::new(path).with_context(|| format!("cannot publish at `{path}`"))?;
	tracing::info!(url = %moq_tokio::RedactedUrl::new(&url), broadcast = %path, "connecting");
	let reports = subscribe.consume();
	let connection = client.with_publisher(&origin).with_subscriber(subscribe).connect(url);

	let publication = origin.create_broadcast(&path).context("cannot create the broadcast")?;
	let mut broadcast = publication.broadcast().clone();
	// Each track reserves its ceiling on the connection's estimate, which the
	// connection then probes to fund: the bandwidth the stream is granted. The
	// catalog claims nothing itself, which would count each track twice.
	let bandwidth = connection.allocator();
	let catalog = moq_mux::catalog::Config::default().with_catalog(Default::default());
	let catalog = moq_scene::publish::Catalog::new(&mut broadcast, catalog)?;
	let clock = catalog.clock();
	let (echo, viewers) = health::echo(&path)?;

	// Each rung opens a throwaway encoder now, so a machine that cannot encode
	// the canvas fails at startup rather than once the encode threads start.
	// The largest also settles which encoder every rung uses, and whether it
	// reads the program on the GPU.
	let framerate = canvas.framerate();
	let sizes = ladder::sizes(moq_video::Size::new(canvas.width, canvas.height));
	let ceilings: Vec<_> = sizes.iter().map(|size| ladder::ceiling(*size, framerate)).collect();
	let mut exports = Vec::with_capacity(sizes.len());
	let mut kind = None;
	for (size, ceiling) in sizes.iter().zip(&ceilings) {
		let mut config = moq_video::encode::Config::new(size.width, size.height, framerate);
		config.bitrate = Some(*ceiling);
		if let Some(kind) = &kind {
			config.kind = Clone::clone(kind);
		}
		let (exporter, rendition) = Exporter::new(&gpu.device, &gpu.queue, &mut config)
			.await
			.with_context(|| format!("cannot encode the canvas at {size}"))?;
		kind.get_or_insert_with(|| config.kind.clone());
		exports.push((config, exporter, rendition));
	}
	// Sources take the program's tier: hardware when the program found it, so a
	// session the GPU refuses fails that source loudly instead of falling to
	// software. When the program could not use hardware, such as NVENC beside an
	// integrated GPU it cannot read, a source may still reach it from the CPU.
	let kind = match &exports[0].0.kind {
		moq_video::encode::Kind::Named(name) if name == "openh264" => moq_video::encode::Kind::Auto,
		_ => moq_video::encode::Kind::Hardware,
	};
	let sources = moq_scene::publish::Publish::new(broadcast.clone(), catalog.clone(), bandwidth.clone(), kind);
	let reserved: Vec<_> = exports
		.iter()
		.map(|(config, ..)| catalog.reserve().with_hls_hint(config.gop.duration(config.framerate)))
		.collect();
	// While the reservations hold, so the solicitation rides the first catalog.
	let mut soliciting = catalog.clone();
	let mut section = soliciting.modify()?;
	section.echo = Some(hang::catalog::Echo::new(echo));
	section.commit()?;
	let mut grants = Vec::with_capacity(sizes.len() + 1);
	let mut rungs = Vec::with_capacity(sizes.len());
	for (index, ((config, exporter, rendition), reserved)) in exports.into_iter().zip(reserved).enumerate() {
		let priority = ladder::priority(hang::catalog::PRIORITY.video, index, sizes.len());
		let producer = ladder::producer(&broadcast, reserved, rendition, priority)?;
		let demand = producer.demand();
		let always = index == 0 && !on_demand;
		let gate = Gate::new(
			&bandwidth,
			demand.clone(),
			ceilings[index],
			ceilings.get(index + 1).copied(),
			always,
		);
		grants.push((demand.name().to_string(), gate.grant()));
		rungs.push(Rung {
			gate,
			exporter,
			producer,
			config,
		});
	}

	let bus = moq_audio::bus::Config::default();
	let mut input = moq_audio::encode::Input::new(bus.sample_rate, bus.layout);
	input.format = moq_audio::Format::F32;
	let mut options = moq_audio::encode::Options::default();
	options.track = Some("audio".into());
	options.config = moq_audio::encode::Config::from_input(moq_audio::encode::Codec::Opus, &input);
	options.config.bitrate = Some(AUDIO_BITRATE);
	let stats = catalog.clone();
	let audio = moq_audio::encode::Producer::new(&mut broadcast, catalog, input, &options)?;
	let audio_reservation = bandwidth.reserve(&audio.demand(), audio.bitrate());
	grants.push((audio.track_name().to_string(), audio_reservation.consume()));

	let epoch = moq_net::Epoch::mint();
	publication
		.announce(moq_net::origin::Route::default().with_epoch(epoch.clone()))
		.context("cannot announce the broadcast")?;

	let feedback = health::Feedback::default();
	let watching = health::Watching::default();
	tokio::spawn(log(
		"viewer feedback",
		feedback.clone().follow(reports.clone(), viewers),
	));
	tokio::spawn(log("relay stats", watching.clone().follow(reports, path.clone())));
	let health = health::Publisher {
		observer: hang::health::Observer::new(path.clone(), Some(epoch)),
		catalog: stats,
		clock: clock.clone(),
		monitor: connection.monitor(),
		feedback,
		watching,
		grants: grants.into_iter().collect(),
	};

	let output = Output {
		path: path.to_string(),
		on_demand,
		_connection: connection,
		publication,
		_audio: audio_reservation,
	};
	Ok((
		output,
		Publish {
			rungs,
			audio,
			clock,
			health,
			sources,
		},
	))
}

/// Run a background reader, logging why it stopped.
async fn log(what: &'static str, task: impl Future<Output = anyhow::Result<()>>) {
	if let Err(err) = task.await {
		tracing::warn!(%err, "stopped reading {what}");
	}
}

/// What the program clock hands a rung's encoder.
enum Video {
	/// A copy of the program on its way to the encoder, to encode at this
	/// bitrate.
	Frame(Pending, moq_net::bandwidth::Rate),
	/// Frames stop until the gate lets the rung encode again: nobody watches,
	/// or the grant cannot fund the rung.
	Step(Step),
	/// The program could not be copied, so the rung stops.
	Failed(moq_video::Error),
}

/// Encode a rung's program frames until the program clock stops. With
/// `opened`, open the encoder first and report how that went on it.
///
/// A failure aborts the rung's track, so viewers see that it failed rather
/// than a track that went away unfinished.
fn encode(
	engine: &Engine,
	opened: Option<tokio::sync::oneshot::Sender<anyhow::Result<()>>>,
	video: mpsc::Receiver<Video>,
	producer: moq_video::encode::Producer,
	config: moq_video::encode::Config,
	zero_copy: bool,
) {
	// The largest rung holds its session while skipping frames, as it was
	// opened to; the others give theirs back while unwatched.
	let mut rendition = Rendition::new(producer, config, opened.is_some());
	if let Some(opened) = opened {
		match rendition.open() {
			Ok(name) => {
				*engine.stats.encoder.lock().unwrap() = Some(name.to_string());
				let _ = opened.send(Ok(()));
			}
			Err(err) => {
				let err = anyhow::Error::new(err).context("video encoder");
				rendition.abort(&format!("{err:#}"));
				let _ = opened.send(Err(err));
				return;
			}
		}
	}
	if let Err(err) = encode_frames(&engine.stats, video, &mut rendition, zero_copy) {
		let err = format!("{err:#}");
		engine.stats.fail(&err);
		rendition.abort(&err);
	}
}

fn encode_frames(
	stats: &Stats,
	video: mpsc::Receiver<Video>,
	rendition: &mut Rendition,
	zero_copy: bool,
) -> anyhow::Result<()> {
	while let Ok(video) = video.recv() {
		let (pending, bitrate) = match video {
			Video::Frame(pending, bitrate) => (pending, bitrate),
			Video::Step(step) => {
				rendition.apply(step).context("video")?;
				continue;
			}
			Video::Failed(err) => return Err(err).context("copying the program"),
		};
		rendition.apply(Step::Encode(bitrate)).context("video encoder")?;
		let start = Instant::now();
		let frame = pending.wait().context("copying the program")?;
		stats
			.copy_us
			.store(start.elapsed().as_micros() as u64, Ordering::Relaxed);
		stats.copied.fetch_add(1, Ordering::Relaxed);
		if !zero_copy {
			stats.read_back.fetch_add(1, Ordering::Relaxed);
		}

		let start = Instant::now();
		rendition.encode(&frame).context("video encoder")?;
		stats
			.encode_us
			.store(start.elapsed().as_micros() as u64, Ordering::Relaxed);
		stats.encoded.fetch_add(1, Ordering::Relaxed);
	}
	Ok(())
}

/// Run a source on its own thread until its feed is dropped.
fn spawn(driver: Driver) {
	let name = format!("source-{}", driver.name());
	let spawned = std::thread::Builder::new()
		.name(name)
		.spawn(move || clocked(driver.run()));
	if let Err(err) = spawned {
		tracing::error!(%err, "cannot start a source thread");
	}
}

/// Run `task` on the calling thread, with its own clock and IO.
fn clocked<F: Future<Output = ()>>(task: F) {
	match tokio::runtime::Builder::new_current_thread().enable_all().build() {
		Ok(runtime) => runtime.block_on(task),
		Err(err) => tracing::error!(%err, "cannot start a runtime"),
	}
}

#[cfg(test)]
mod tests {
	use moq_mux::catalog::Stream;

	use super::*;

	/// Bounds each step, so a stall fails rather than hangs.
	const TIMEOUT: Duration = Duration::from_secs(30);

	/// The program a studio publishes reaches a relay as the picture it composed,
	/// with its audio alongside.
	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "needs a Vulkan device and an H.264 encoder; run by just rs render-sw"]
	async fn the_program_reaches_a_relay() {
		let _ = moq_tokio::crypto::install_default();
		let fixture = moq_relay::test_relay().await.expect("test relay");
		let ready = fixture.relay.ready();
		let relay = fixture.relay.cluster().origin.consume();
		tokio::spawn(fixture.relay.run());
		ready.wait().await.expect("relay ready");

		let mut client = moq_tokio::connect::Config::default();
		client.tls.fingerprint = vec![fixture.fingerprint.clone()];
		let client = client.init(Default::default()).expect("client");

		let dir = tempfile::tempdir().unwrap();
		let mut collection = Collection::new();
		collection.canvas = moq_scene::Canvas {
			// Two rungs: the canvas, and two thirds of it at the 180-line floor.
			width: 400,
			height: 270,
			framerate: 30,
		};
		collection.sources.insert(
			"red".into(),
			moq_scene::Source::Color {
				color: moq_scene::Rgba([255, 0, 0, 255]),
			},
		);
		collection.scenes[0].items.push(moq_scene::Item {
			source: "red".into(),
			rect: moq_scene::Rect::fill(&collection.canvas),
			visible: true,
			filters: Vec::new(),
		});

		let gpu = Gpu::new().await.expect("a GPU");
		let (autosave, _) = Autosave::open(dir.path().join("show.toml")).unwrap();
		let connect = Some((client, fixture.url.clone()));
		let on_demand = false;
		let loaded = autosave::Loaded {
			collection,
			history: History::new(),
			dropped: 0,
		};
		let engine = Engine::start(gpu, autosave, loaded, connect, "studio", on_demand)
			.await
			.expect("the studio starts");
		assert!(
			engine.stats.encoder.lock().unwrap().is_some(),
			"the program holds its encoder before any source opens one"
		);

		let broadcast = kio::clock::timeout(TIMEOUT, relay.follow("studio", None).unwrap().available())
			.await
			.expect("the relay routes the program")
			.expect("broadcast");
		let format = moq_mux::catalog::CatalogFormat::default();
		let mut catalogs = moq_mux::catalog::Consumer::<()>::new(&broadcast, format)
			.await
			.expect("catalog");
		let catalog = catalogs.next().await.expect("catalog").expect("a catalog");
		let mut sizes: Vec<_> = catalog
			.video
			.renditions
			.values()
			.map(|config| (config.coded_width, config.coded_height))
			.collect();
		sizes.sort_unstable_by(|a, b| b.cmp(a));
		assert_eq!(
			sizes,
			[(Some(400), Some(270)), (Some(266), Some(180))],
			"the program's ladder, and only it, is the video"
		);
		// Viewers are asked for feedback beneath the broadcast from its first catalog.
		let echo = catalog.echo.as_ref().expect("the catalog solicits feedback");
		assert_eq!(echo.path.as_str(), "studio/viewers");
		// The smaller rung encodes once watched, scaled from the canvas.
		for (video, config) in catalog.video.enabled() {
			let size = moq_video::Size::new(config.coded_width.unwrap(), config.coded_height.unwrap());
			let mut options = moq_video::decode::Options::new();
			options.start = moq_video::decode::Start::Latest;
			let mut decoder = moq_video::decode::Consumer::new(&broadcast, config, video.clone(), options)
				.await
				.expect("video decoder");
			// The canvas is black until the color source delivers its first
			// picture, so wait for the composed one rather than take the first.
			kio::clock::timeout(TIMEOUT, async {
				loop {
					let frame = match decoder.read().await.expect("decode") {
						Some(moq_video::decode::Read::Frame(frame)) => frame,
						Some(_) => continue,
						None => panic!("the video ended"),
					};
					assert_eq!(frame.size(), size);
					let rgba = frame.surface.to_rgba(&Default::default()).expect("rgba");
					let center = ((size.height / 2 * size.width + size.width / 2) * 4) as usize;
					let [r, g, b, _] = rgba.data()[center..center + 4] else {
						unreachable!()
					};
					// H.264 and its 4:2:0 round trip shift a primary a little.
					if r > 200 && g < 60 && b < 60 {
						return;
					}
				}
			})
			.await
			.expect("the composed picture arrives");
		}

		// The program reaches the encoder as a GPU surface when the encoder
		// reads one, and is read back frame by frame only when it does not.
		let stats = &engine.stats;
		assert!(stats.copied.load(Ordering::Relaxed) > 0);
		let read_back = stats.read_back.load(Ordering::Relaxed);
		if stats.zero_copy.load(Ordering::Relaxed) {
			assert_eq!(read_back, 0, "a zero-copy program was read back");
		} else {
			assert!(read_back > 0, "the program was neither exported nor read back");
		}

		// The color source is listed beside the program as its own still.
		let mut listings = moq_mux::catalog::Consumer::<moq_scene::publish::Ext>::new(&broadcast, format)
			.await
			.expect("catalog");
		kio::clock::timeout(TIMEOUT, async {
			loop {
				let catalog = listings.next().await.expect("catalog").expect("a catalog");
				if let Some(red) = catalog.ext.studio.sources.get("red") {
					assert_eq!(red.binary.tracks.len(), 1, "{red:?}");
					return;
				}
			}
		})
		.await
		.expect("the source is listed");

		let (audio, config) = catalog.audio.enabled().next().expect("an audio rendition");
		let mut options = moq_audio::decode::Options::new();
		options.start = moq_audio::decode::Start::Latest;
		let mut decoder = moq_audio::decode::Consumer::new(&broadcast, config, audio.clone(), options)
			.await
			.expect("audio decoder");
		kio::clock::timeout(TIMEOUT, async {
			loop {
				match decoder.read().await.expect("decode") {
					Some(moq_audio::decode::Read::Frame(_)) => return,
					Some(_) => continue,
					None => panic!("the audio ended"),
				}
			}
		})
		.await
		.expect("sound arrives");

		// An edit, and then its undo, reach the scene file and the history
		// beside it through the autosave the studio runs.
		let path = dir.path().join("show.toml");
		let history = dir.path().join("show.history.toml");
		let read = || {
			let collection = std::fs::read_to_string(&path).ok()?;
			let history = std::fs::read_to_string(&history).ok()?;
			Some(autosave::Text { collection, history })
		};
		let saved = |text: autosave::Text| {
			let read = &read;
			kio::clock::timeout(TIMEOUT, async move {
				while read().as_ref() != Some(&text) {
					kio::clock::sleep(autosave::QUIET / 10).await;
				}
			})
		};
		let edited = {
			let mut state = engine.state.lock().unwrap();
			let mut draft = state.collection().draft("Add scene `Edited`");
			draft.insert_scene(1, moq_scene::Scene::new("Edited")).unwrap();
			state.commit(draft, None).unwrap();
			autosave::Text::new(state.collection(), state.history()).unwrap()
		};
		saved(edited).await.expect("the edit is saved");
		let undone = {
			let mut state = engine.state.lock().unwrap();
			state.undo().unwrap().expect("an edit to undo");
			autosave::Text::new(state.collection(), state.history()).unwrap()
		};
		saved(undone).await.expect("the undo is saved");

		// Health sees the relay and what each track of the program writes.
		kio::clock::timeout(TIMEOUT, async {
			loop {
				{
					let report = engine.health.lock().unwrap();
					// Both rungs and the audio; the smaller rung sends only while watched.
					let sending = report.tracks.len() == 3
						&& report
							.tracks
							.iter()
							.filter(|track| track.sent.is_some_and(|sent| sent.as_bps() > 0))
							.count() >= 2;
					if report.relay == Some(health::Relay::Connected) && sending {
						return;
					}
				}
				kio::clock::sleep(health::INTERVAL).await;
			}
		})
		.await
		.expect("health grades the program's tracks");
	}
}
