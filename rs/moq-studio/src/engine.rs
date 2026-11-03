//! Everything behind the window: the GPU, the sources, the program clock that
//! draws the canvas and mixes the audio, and the broadcast it publishes: the
//! program, and each local source's own track beside it.
//!
//! Threads:
//! - **program** draws preview and program once per canvas frame and hands the
//!   program to the encoder, stamped with the program clock.
//! - **audio** reads the program bus on the same clock and publishes it.
//! - **encode** waits for each program copy and encodes it.
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
use moq_scene::source::{self, Driver, Sources};
use moq_scene::{Collection, Compositor, Draft, History, wgpu};
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
	/// Program frames handed to the encoder.
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
	/// Program frames encoded.
	pub encoded: AtomicU64,
	/// Microseconds the last encode took.
	pub encode_us: AtomicU64,
	/// Whether any viewer is subscribed to the video.
	pub watched: AtomicBool,
	/// The encoder backend, once one opened.
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
	/// Whether the program video is copied and encoded only while someone
	/// watches, rather than all the time.
	pub on_demand: bool,
	/// A connection to each relay, which reconnects on its own and dials only
	/// while held. The Health panel reads their states.
	_connections: Vec<moq_tokio::Connection>,
	publication: moq_net::origin::Publication,
}

/// A relay the program is published to.
#[derive(Clone)]
pub struct Relay {
	pub client: moq_tokio::Client,
	pub url: url::Url,
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
pub const AUDIO_BITRATE: moq_net::bandwidth::Rate = moq_net::bandwidth::Rate::from_kbps(128);

/// How often the audio thread reads the bus. Short enough that a read is never
/// close to the bus's buffering, long enough to cost nothing.
const AUDIO_PERIOD: Duration = Duration::from_millis(10);

/// Everything starting the engine takes besides the scene file, which guided
/// setup holds back until the creator has settled on a collection.
#[derive(Clone)]
pub struct Launch {
	pub gpu: Gpu,
	/// The relays to publish to, the preferred first; none publishes nothing.
	pub relays: Vec<Relay>,
	/// The broadcast path the program is published at.
	pub broadcast: String,
	/// Whether the program video encodes only while someone watches.
	pub on_demand: bool,
}

impl Engine {
	/// Open the `loaded` collection and history on `launch.gpu`, start the
	/// program clock, and save both to `autosave`'s files as they change,
	/// starting at once.
	/// Publish the program at `launch.broadcast` on every one of
	/// `launch.relays`, preferred first, encoding its video only while someone
	/// watches when `launch.on_demand`.
	///
	/// Call inside the tokio runtime that serves the relay connections.
	pub async fn start(launch: Launch, autosave: Autosave, loaded: autosave::Loaded) -> anyhow::Result<Arc<Self>> {
		let Launch {
			gpu,
			relays,
			broadcast,
			on_demand,
		} = launch;
		let bus = moq_audio::bus::Bus::new(Default::default())?;
		let subscribe = moq_tokio::origin::spawn();
		let sources = subscribe.consume();
		let autosave::Loaded {
			collection,
			history,
			dropped,
		} = loaded;
		let canvas = collection.canvas;

		let (output, origin, publish) = if relays.is_empty() {
			(None, Some(subscribe), None)
		} else {
			let (output, publish) = publish(&gpu, relays, subscribe, &broadcast, canvas, on_demand).await?;
			(Some(output), None, Some(publish))
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
				video,
				audio,
				config,
				exporter,
				clock,
				health,
				sources: _,
			}) => {
				let zero_copy = exporter.route() == Route::Gpu;
				engine.stats.zero_copy.store(zero_copy, Ordering::Relaxed);
				// One more than the exporter's pool, for a pause behind a full pool.
				let (frames, pending) = mpsc::sync_channel(Exporter::DEPTH + 1);
				tokio::spawn(watch(video.demand(), engine.clone()));
				let encoder = engine.clone();
				let (opened, open) = tokio::sync::oneshot::channel();
				std::thread::Builder::new()
					.name("encode".into())
					.spawn(move || encode(&encoder, opened, pending, video, config, zero_copy))?;
				// Before the program clock starts the sources, so the program holds
				// its encode session even while unwatched, and a source past the
				// GPU's session cap is the one that fails.
				open.await.context("the encode thread stopped")??;
				(Some((frames, exporter)), Some(audio), Some(clock), Some(health))
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
		clocked("program".into(), async move || {
			program.run_program(start, epoch, video).await
		})?;
		let mix = engine.clone();
		clocked("audio".into(), async move || mix.run_audio(start, epoch, audio).await)?;
		let save = engine.clone();
		clocked("autosave".into(), async move || {
			save.autosave.run(|| save.serialize()).await
		})?;
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
		let on_demand = self.output.as_ref().is_some_and(|output| output.on_demand);
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
			rendered: stats.rendered.load(Ordering::Relaxed),
			dropped: stats.dropped.load(Ordering::Relaxed),
			encoded: stats.encoded.load(Ordering::Relaxed),
			encoding: self.output.is_some() && (!on_demand || watched),
			watched,
			encoder: stats.encoder.lock().unwrap().clone(),
			failed: stats.error.lock().unwrap().clone(),
			stalled,
		}
	}

	/// Draw a canvas frame per tick of the program clock, and hand the program
	/// to the encoder, stamped `epoch` past `start`: every frame, or only while
	/// anyone watches when the output is on demand.
	///
	/// Drawing outlives the encoder: a failed output stops copying the program,
	/// and the preview and program keep moving.
	async fn run_program(
		&self,
		start: Instant,
		epoch: Duration,
		mut video: Option<(mpsc::SyncSender<Video>, Exporter)>,
	) {
		let canvas = self.state.lock().unwrap().collection().canvas;
		let period = Duration::from_secs(1) / canvas.framerate;
		let mut clock = kio::clock::interval_at(start, period);
		clock.set_missed_tick_behavior(kio::clock::MissedTickBehavior::Skip);
		let on_demand = self.output.as_ref().is_some_and(|output| output.on_demand);
		let mut encoding = false;

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

			let Some((sender, exporter)) = &mut video else { continue };
			let wanted = !on_demand || self.stats.watched.load(Ordering::Relaxed);
			let was = std::mem::replace(&mut encoding, wanted);
			if !encoding {
				// Behind every frame already sent, so the break lands after them.
				// A send fails only once the encoder stopped, having reported why.
				if was && sender.send(Video::Pause).is_err() {
					video = None;
				}
				continue;
			}
			let Ok(timestamp) = moq_net::Timestamp::try_from(epoch + tick.saturating_duration_since(start)) else {
				continue;
			};
			match exporter.copy(&program, timestamp) {
				// The pool bounds the frames in flight, so this waits only behind
				// a pause the encoder has not reached yet.
				Ok(Some(pending)) => {
					if sender.send(Video::Frame(pending)).is_err() {
						video = None;
					}
				}
				Ok(None) => {
					self.stats.dropped.fetch_add(1, Ordering::Relaxed);
				}
				// The canvas and the exporter share a size and format, so this is
				// the GPU failing: the video stops, and the canvas keeps drawing.
				Err(err) => {
					let _ = sender.send(Video::Failed(err));
					video = None;
				}
			}
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
	video: moq_video::encode::Producer,
	config: moq_video::encode::Config,
	exporter: Exporter,
	audio: moq_audio::encode::Producer,
	/// The catalog's clock, which the codec producers write timestamps
	/// against verbatim.
	clock: moq_mux::Clock,
	/// What the health sampler grades.
	health: health::Publisher,
	/// Where each local source publishes its own track.
	sources: moq_scene::publish::Publish,
}

/// Announce the program broadcast under one epoch, which also carries each
/// local source's own track, and connect to every relay, each also carrying
/// `subscribe`'s broadcast sources.
async fn publish(
	gpu: &Gpu,
	relays: Vec<Relay>,
	subscribe: moq_net::origin::Producer,
	path: &str,
	canvas: moq_scene::Canvas,
	on_demand: bool,
) -> anyhow::Result<(Output, Publish)> {
	let origin = moq_tokio::origin::spawn();
	let path = moq_net::Path::new(path).with_context(|| format!("cannot publish at `{path}`"))?;
	let reports = subscribe.consume();
	tracing::info!(broadcast = %path, "publishing");
	let (connections, links) = connect(relays, &origin, &subscribe);

	let publication = origin.create_broadcast(&path).context("cannot create the broadcast")?;
	let mut broadcast = publication.broadcast().clone();
	// Each track claims its bitrate on the first relay's estimate, which that
	// connection then probes to fund: the bandwidth the stream is granted. An
	// allocator spans one connection, so the failover's link is never probed.
	let bandwidth = connections[0].allocator();
	let catalog = moq_mux::catalog::Config::default()
		.with_bandwidth(bandwidth.clone())
		.with_catalog(Default::default());
	let catalog = moq_scene::publish::Catalog::new(&mut broadcast, catalog)?;
	let clock = catalog.clock();
	let (echo, viewers) = health::echo(&path)?;

	let mut config = moq_video::encode::Config::new(canvas.width, canvas.height, canvas.framerate());
	// Opens a throwaway encoder now, so a machine that cannot encode the
	// canvas fails at startup rather than once the encode thread starts. It
	// also settles whether that encoder reads the program on the GPU.
	let (exporter, rendition) = Exporter::new(&gpu.device, &gpu.queue, &mut config)
		.await
		.context("cannot encode the canvas")?;
	// Sources take the program's tier: hardware when the program found it, so a
	// session the GPU refuses fails that source loudly instead of falling to
	// software. When the program could not use hardware, such as NVENC beside an
	// integrated GPU it cannot read, a source may still reach it from the CPU.
	let kind = match &config.kind {
		moq_video::encode::Kind::Named(name) if name == "openh264" => moq_video::encode::Kind::Auto,
		_ => moq_video::encode::Kind::Hardware,
	};
	let sources = moq_scene::publish::Publish::new(broadcast.clone(), catalog.clone(), bandwidth, kind);
	let reserved = catalog.reserve().with_hls_hint(config.gop.duration(config.framerate));
	// While the reservation holds, so the solicitation rides the first catalog.
	let mut soliciting = catalog.clone();
	let mut section = soliciting.modify()?;
	section.echo = Some(hang::catalog::Echo::new(echo));
	section.commit()?;
	let video = moq_video::encode::Producer::new(broadcast.clone(), reserved, rendition)?;

	let bus = moq_audio::bus::Config::default();
	let mut input = moq_audio::encode::Input::new(bus.sample_rate, bus.layout);
	input.format = moq_audio::Format::F32;
	let mut options = moq_audio::encode::Options::default();
	options.track = Some("audio".into());
	options.config = moq_audio::encode::Config::from_input(moq_audio::encode::Codec::Opus, &input);
	options.config.bitrate = Some(AUDIO_BITRATE);
	let stats = catalog.clone();
	let audio = moq_audio::encode::Producer::new(&mut broadcast, catalog, input, &options)?;

	// One epoch on every relay, so a viewer moving between them resumes
	// mid-group rather than restarting.
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
		relays: links,
		feedback,
		watching,
	};

	let output = Output {
		path: path.to_string(),
		on_demand,
		_connections: connections,
		publication,
	};
	Ok((
		output,
		Publish {
			video,
			config,
			exporter,
			audio,
			clock,
			health,
			sources,
		},
	))
}

/// Connect to every relay, publishing `origin` to each and feeding `subscribe`
/// the broadcast sources of each, and watch each connection for the Health
/// panel.
fn connect(
	relays: Vec<Relay>,
	origin: &moq_net::origin::Producer,
	subscribe: &moq_net::origin::Producer,
) -> (Vec<moq_tokio::Connection>, Vec<health::Link>) {
	relays
		.into_iter()
		.enumerate()
		.map(|(rank, Relay { client, url })| {
			tracing::info!(url = %moq_tokio::RedactedUrl::new(&url), "connecting");
			// Only the scheme, host, and port: a path can carry a credential, and
			// the panel may be on a shared screen. A URL without a host, such as
			// a unix socket's, keeps its path, which is the address.
			let endpoint = match url.host_str() {
				Some(host) => match url.port() {
					Some(port) => format!("{}://{host}:{port}", url.scheme()),
					None => format!("{}://{host}", url.scheme()),
				},
				None => format!("{}:{}", url.scheme(), url.path()),
			};
			// Among routes of one instance of a broadcast source, the first
			// relay's wins while it has one. A newer instance wins first,
			// whichever relay it comes through.
			let preference = match rank {
				0 => moq_net::origin::Preference::default(),
				_ => moq_net::origin::Preference::new(-1),
			};
			let sources = subscribe.clone().with_preference(preference);
			let connection = client.with_publisher(origin).with_subscriber(sources).connect(url);
			let monitor = connection.monitor();
			(connection, health::Link { endpoint, monitor })
		})
		.unzip()
}

/// Run a background reader, logging why it stopped.
async fn log(what: &'static str, task: impl Future<Output = anyhow::Result<()>>) {
	if let Err(err) = task.await {
		tracing::warn!(%err, "stopped reading {what}");
	}
}

/// Track whether anyone watches the video, for the Output panel, and so an
/// on-demand program is only copied and encoded while someone does.
async fn watch(mut demand: moq_net::track::Demand, engine: Arc<Engine>) {
	loop {
		match demand.wanted_changed().await {
			Ok(wanted) => engine.stats.watched.store(wanted.is_some(), Ordering::Relaxed),
			Err(err) => {
				engine.stats.watched.store(false, Ordering::Relaxed);
				engine.stats.fail(format!("video: {err}"));
				return;
			}
		}
	}
}

/// What the program clock hands the encoder.
enum Video {
	/// A copy of the program on its way to the encoder.
	Frame(Pending),
	/// The last viewer of an on-demand program left, so frames stop until one
	/// returns.
	Pause,
	/// The program could not be copied, so the video stops.
	Failed(moq_video::Error),
}

/// Open the program's encoder, reporting how that went on `opened`, then
/// encode program frames until the program clock stops.
///
/// A failure aborts the video track, so viewers see that it failed rather
/// than a track that went away unfinished.
fn encode(
	engine: &Engine,
	opened: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
	video: mpsc::Receiver<Video>,
	mut producer: moq_video::encode::Producer,
	config: moq_video::encode::Config,
	zero_copy: bool,
) {
	let mut encoder = match moq_video::encode::Encoder::new(&config) {
		Ok(encoder) => encoder,
		Err(err) => {
			producer.abort(moq_net::StreamError::Internal.into());
			let _ = opened.send(Err(anyhow::Error::new(err).context("video encoder")));
			return;
		}
	};
	*engine.stats.encoder.lock().unwrap() = Some(encoder.name().to_string());
	let _ = opened.send(Ok(()));

	let result = encode_frames(&engine.stats, video, &mut producer, &mut encoder, zero_copy).and_then(|()| {
		producer.finish()?;
		Ok(())
	});
	if let Err(err) = result {
		engine.stats.fail(format!("{err:#}"));
		producer.abort(moq_net::StreamError::Internal.into());
	}
}

fn encode_frames(
	stats: &Stats,
	video: mpsc::Receiver<Video>,
	producer: &mut moq_video::encode::Producer,
	encoder: &mut moq_video::encode::Encoder,
	zero_copy: bool,
) -> anyhow::Result<()> {
	let mut paused = false;

	while let Ok(video) = video.recv() {
		let pending = match video {
			Video::Frame(pending) => pending,
			// Marks the gap, so a viewer arriving during it is not served the
			// last group before it as if it were live.
			Video::Pause => {
				producer.discontinuity().context("video")?;
				paused = true;
				continue;
			}
			Video::Failed(err) => return Err(err).context("copying the program"),
		};
		let start = Instant::now();
		let frame = pending.wait().context("copying the program")?;
		stats
			.copy_us
			.store(start.elapsed().as_micros() as u64, Ordering::Relaxed);
		stats.copied.fetch_add(1, Ordering::Relaxed);
		if !zero_copy {
			stats.read_back.fetch_add(1, Ordering::Relaxed);
		}

		// A viewer arriving after a break starts from a keyframe rather than
		// waiting out the rest of a group nobody received the start of.
		if std::mem::take(&mut paused) {
			encoder.cut().context("video encoder")?;
		}

		let start = Instant::now();
		let encoded = encoder.encode(&frame).context("video encoder")?;
		producer.publish(&encoded).context("video")?;
		stats
			.encode_us
			.store(start.elapsed().as_micros() as u64, Ordering::Relaxed);
		stats.encoded.fetch_add(1, Ordering::Relaxed);
	}
	Ok(())
}

/// Run a source on its own thread until its feed is dropped, or fail the
/// source when its thread cannot start.
fn spawn(driver: Driver) {
	// Handed over once the thread exists, since a failed spawn drops what it
	// was given and the driver must stay here to report why.
	let (handoff, received) = mpsc::sync_channel::<Driver>(1);
	let name = format!("source-{}", driver.name());
	let started = clocked(name, async move || {
		if let Ok(driver) = received.recv() {
			driver.run().await;
		}
	});
	match started {
		Ok(()) => {
			if let Err(mpsc::SendError(driver)) = handoff.send(driver) {
				driver.fail("the source thread stopped");
			}
		}
		Err(err) => driver.fail(format!("{err:#}")),
	}
}

/// Start a thread called `name` that runs `task` with its own clock and IO,
/// returning why if either cannot start.
fn clocked(name: String, task: impl AsyncFnOnce() + Send + 'static) -> anyhow::Result<()> {
	// The thread starts before its runtime is built and handed over, since a
	// runtime dropped in the async caller, as a refused spawn would, panics.
	let (handoff, received) = mpsc::sync_channel::<tokio::runtime::Runtime>(1);
	std::thread::Builder::new()
		.name(name.clone())
		.spawn(move || {
			if let Ok(runtime) = received.recv() {
				runtime.block_on(task());
			}
		})
		.with_context(|| format!("cannot start the {name} thread"))?;
	let runtime = tokio::runtime::Builder::new_current_thread()
		.enable_all()
		.build()
		.with_context(|| format!("cannot start the {name} runtime"))?;
	if let Err(mpsc::SendError(runtime)) = handoff.send(runtime) {
		runtime.shutdown_background();
		anyhow::bail!("the {name} thread stopped");
	}
	Ok(())
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
			width: 320,
			height: 180,
			framerate: 30,
		};
		collection.sources.insert(
			"red".into(),
			moq_scene::Source::Color {
				color: moq_scene::Rgba([255, 0, 0, 255]),
			},
		);
		collection.scenes[0]
			.items
			.push(moq_scene::Item::new("red", moq_scene::Rect::fill(&collection.canvas)));

		let launch = Launch {
			gpu: Gpu::new().await.expect("a GPU"),
			relays: vec![Relay {
				client,
				url: fixture.url.clone(),
			}],
			broadcast: "studio".into(),
			on_demand: false,
		};
		let (autosave, _) = Autosave::open(dir.path().join("show.toml")).unwrap();
		let loaded = autosave::Loaded {
			collection,
			history: History::new(),
			dropped: 0,
		};
		let engine = Engine::start(launch, autosave, loaded)
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
		assert_eq!(catalog.video.renditions.len(), 1, "only the program is a rendition");
		// Viewers are asked for feedback beneath the broadcast from its first catalog.
		let echo = catalog.echo.as_ref().expect("the catalog solicits feedback");
		assert_eq!(echo.path.as_str(), "studio/viewers");
		let (video, config) = catalog.video.enabled().next().expect("a video rendition");
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
				assert_eq!(frame.size(), moq_video::Size::new(320, 180));
				let rgba = frame.surface.to_rgba(&Default::default()).expect("rgba");
				let center = (90 * 320 + 160) * 4;
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
					let sending = report.tracks.len() == 2
						&& report
							.tracks
							.iter()
							.all(|track| track.sent.is_some_and(|sent| sent.as_bps() > 0));
					let connected = matches!(&report.relays[..], [relay] if relay.state == health::State::Connected);
					if connected && sending {
						return;
					}
				}
				kio::clock::sleep(health::INTERVAL).await;
			}
		})
		.await
		.expect("health grades the program's tracks");
	}

	/// With a failover relay, both relays route the program under its one
	/// epoch over the one protocol version, so a viewer moving between them
	/// resumes. A broadcast source only the failover has still reaches the
	/// studio, and a newer instance of a source behind the failover replaces
	/// the first relay's.
	#[tokio::test(flavor = "multi_thread")]
	async fn every_relay_routes_one_epoch() {
		let _ = moq_tokio::crypto::install_default();
		let version: moq_net::Version = "moq-lite-07-wip".parse().unwrap();
		let mut relays = Vec::new();
		let mut routes = Vec::new();
		let mut guests = Vec::new();
		for _ in 0..2 {
			let fixture = moq_relay::test_relay().await.expect("test relay");
			let ready = fixture.relay.ready();
			routes.push(fixture.relay.cluster().origin.consume());
			tokio::spawn(fixture.relay.run());
			ready.wait().await.expect("relay ready");
			let mut client = moq_tokio::connect::Config::default();
			client.tls.fingerprint = vec![fixture.fingerprint.clone()];
			client.version = vec![version];
			let client = client.init(Default::default()).expect("client");
			guests.push((client.clone(), fixture.url.clone()));
			relays.push(Relay {
				client,
				url: fixture.url,
			});
		}

		let origin = moq_tokio::origin::spawn();
		let subscribe = moq_tokio::origin::spawn();
		let (_connections, links) = connect(relays, &origin, &subscribe);
		let publication = origin.create_broadcast("studio").unwrap();
		let epoch = moq_net::Epoch::mint();
		publication
			.announce(moq_net::origin::Route::default().with_epoch(epoch.clone()))
			.unwrap();

		// Publish `path` under `epoch` through one relay's guest.
		let publish = |path: &str, epoch: Option<&moq_net::Epoch>, (client, url): &(moq_tokio::Client, url::Url)| {
			let origin = moq_tokio::origin::spawn();
			let source = origin.create_broadcast(path).unwrap();
			let route = moq_net::origin::Route::default();
			let route = match epoch {
				Some(epoch) => route.with_epoch(epoch.clone()),
				None => route,
			};
			source.announce(route).unwrap();
			let connection = client.clone().with_publisher(&origin).connect(url.clone());
			(origin, source, connection)
		};
		let _guest = publish("guest", None, &guests[1]);
		let mut sources = subscribe.consume().announced();

		let routed = async {
			for relay in &routes {
				let mut announced = relay.announced();
				loop {
					match announced.next().await.expect("announcements") {
						moq_net::announce::Event::Start(announce) if announce.prefix.as_str() == "studio" => {
							assert_eq!(announce.route.epoch, Some(epoch.clone()));
							break;
						}
						_ => continue,
					}
				}
			}
			for link in &links {
				let mut state = link.monitor.state();
				while state.version != Some(version) {
					state = link
						.monitor
						.state_changed(state.revision)
						.await
						.expect("the connection");
				}
			}
			subscribe
				.consume()
				.follow("guest", None)
				.unwrap()
				.available()
				.await
				.expect("the source");

			// The newest instance of a source wins before the first relay's rank,
			// so one restarted behind the failover replaces the first relay's.
			let older: moq_net::Epoch = "01900000-0000-7000-8000-000000000000".parse().unwrap();
			let newer: moq_net::Epoch = "01910000-0000-7000-8000-000000000000".parse().unwrap();
			let _first = publish("show", Some(&older), &guests[0]);
			routed_at(&mut sources, &older).await;
			let _restarted = publish("show", Some(&newer), &guests[1]);
			routed_at(&mut sources, &newer).await;
		};
		kio::clock::timeout(TIMEOUT, routed)
			.await
			.expect("both relays route the program, the failover the sources");
	}

	/// Wait until `announced` routes `show` under `epoch`.
	async fn routed_at(announced: &mut moq_net::announce::Consumer, epoch: &moq_net::Epoch) {
		use moq_net::announce::Event;
		loop {
			let announce = match announced.next().await.expect("announcements") {
				Event::Start(announce) | Event::Update(announce) | Event::Restart(announce) => announce,
				Event::Takeover { announce, .. } => announce,
				Event::End(_) => continue,
			};
			if announce.prefix.as_str() == "show" && announce.route.epoch.as_ref() == Some(epoch) {
				return;
			}
		}
	}
}
