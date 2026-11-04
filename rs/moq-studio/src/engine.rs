//! Everything behind the window: the GPU, the sources, the program clock that
//! draws the canvas and mixes the audio, and the broadcast it publishes: the
//! program, and each local source's own track beside it.
//!
//! Threads:
//! - **program** draws preview and program once per canvas frame and hands the
//!   program to the encoder, stamped with the program clock.
//! - **audio** reads the program bus on the same clock and publishes it.
//! - **encode**: one per program rendition, which waits for each program copy
//!   and encodes it. A new canvas size or frame rate starts a new rendition.
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
use moq_scene::{Canvas, Collection, Compositor, Draft, Edit, History, wgpu};
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
		let mut collection = self.collection.clone();
		collection.apply(&edit)?;
		self.adopt(collection, std::slice::from_ref(&edit))?;
		self.history.record(edit, gesture);
		self.edited();
		Ok(())
	}

	/// Undo the newest edit, returning its label, or `None` with none left.
	pub fn undo(&mut self) -> anyhow::Result<Option<String>> {
		let Some(position) = self.history.position().checked_sub(1) else {
			return Ok(None);
		};
		let label = self.label(position);
		self.seek(position)?;
		Ok(label)
	}

	/// Redo the oldest undone edit, returning its label, or `None` with none left.
	pub fn redo(&mut self) -> anyhow::Result<Option<String>> {
		let position = self.history.position();
		let Some(label) = self.label(position) else {
			return Ok(None);
		};
		self.seek(position + 1)?;
		Ok(Some(label))
	}

	fn label(&self, index: usize) -> Option<String> {
		self.history.entries().nth(index).map(|edit| edit.label.clone())
	}

	/// Undo or redo to just after the history's `position`th entry.
	pub fn seek(&mut self, position: usize) -> anyhow::Result<()> {
		let mut history = self.history.clone();
		let mut collection = self.collection.clone();
		history.seek(&mut collection, position)?;
		// What the seek did, in order: the entries it redid, or the inverse of
		// those it undid, newest first.
		let from = self.history.position();
		let entries: Vec<&Edit> = self.history.entries().collect();
		let made: Vec<Edit> = if position < from {
			entries[position..from]
				.iter()
				.rev()
				.map(|edit| edit.inverse())
				.collect()
		} else {
			entries[from..position].iter().map(|&edit| edit.clone()).collect()
		};
		self.adopt(collection, &made)?;
		self.history = history;
		self.edited();
		Ok(())
	}

	/// Make `collection`, which the edits `made` led to from the current one,
	/// the one drawn, once the compositor takes its canvas. The scenes in
	/// preview and on program follow the renames and removals `made` holds,
	/// each edit read against the collection it was made to, and any
	/// `collection` lacks is dropped.
	fn adopt(&mut self, collection: Collection, made: &[Edit]) -> anyhow::Result<()> {
		let mut before = self.collection.clone();
		let mut renames = Vec::with_capacity(made.len());
		for edit in made {
			renames.push(before.renames(edit));
			before.apply(edit)?;
		}
		self.compositor.set_canvas(collection.canvas)?;
		for renames in &renames {
			self.compositor.rename(renames);
		}
		self.compositor.retain(&collection);
		self.collection = collection;
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
		// A scene in preview or on program that the file no longer has, such as
		// one renamed by hand, leaves it until another is picked.
		self.adopt(collection, &[])?;
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
	/// Whether the program is handed to an encoder now: not while on demand
	/// and unwatched, nor while a new rendition opens.
	pub encoding: AtomicBool,
	/// The encoder backend, once the program's current rendition opened one.
	encoder: Mutex<EncoderName>,
	/// Why the video stopped, if it did, until a new rendition starts.
	video_error: Mutex<Option<String>>,
	/// Why the audio stopped, if it did.
	audio_error: Mutex<Option<String>>,
}

/// The encoder backend the program's current rendition opened, if it has.
#[derive(Default)]
struct EncoderName {
	/// Counts the program's renditions, so one a resize retired while its
	/// encoder was still opening cannot report that encoder afterwards.
	rendition: u64,
	name: Option<String>,
}

impl Stats {
	/// The encoder backend, once the program's current rendition opened one.
	pub fn encoder(&self) -> Option<String> {
		self.encoder.lock().unwrap().name.clone()
	}

	/// The number of the program's current rendition, which its encode thread
	/// reports its encoder under.
	fn rendition(&self) -> u64 {
		self.encoder.lock().unwrap().rendition
	}

	/// Retire the current rendition, so its encoder is no longer reported, even
	/// one that opens afterwards.
	fn retire_encoder(&self) {
		let mut encoder = self.encoder.lock().unwrap();
		encoder.rendition += 1;
		encoder.name = None;
	}

	/// Report `name` as the program's encoder, unless `rendition` was retired.
	fn opened_encoder(&self, rendition: u64, name: &str) {
		let mut encoder = self.encoder.lock().unwrap();
		if encoder.rendition == rendition {
			encoder.name = Some(name.to_string());
		}
	}

	/// Why the output stopped, if it did.
	pub fn error(&self) -> Option<String> {
		let video = self.video_error.lock().unwrap().clone();
		let audio = self.audio_error.lock().unwrap().clone();
		match (video, audio) {
			(Some(video), Some(audio)) => Some(format!("{video}; {audio}")),
			(video, audio) => video.or(audio),
		}
	}

	fn fail_video(&self, err: impl std::fmt::Display) {
		tracing::error!(%err, "video stopped");
		*self.video_error.lock().unwrap() = Some(format!("video: {err}"));
	}

	fn fail_audio(&self, err: impl std::fmt::Display) {
		tracing::error!(%err, "audio stopped");
		*self.audio_error.lock().unwrap() = Some(format!("audio: {err}"));
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
	pub stats: Arc<Stats>,
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
		let stats = Arc::new(Stats::default());

		let (output, origin, publish) = if relays.is_empty() {
			(None, Some(subscribe), None)
		} else {
			let (output, publish) = publish(&gpu, &stats, relays, subscribe, &broadcast, canvas, on_demand).await?;
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
			stats,
			health: Mutex::default(),
			bus,
			output,
			_origin: origin,
		});

		let (video, audio, clock, publisher) = match publish {
			Some(Publish {
				renditions,
				rendition,
				audio,
				clock,
				health,
				sources: _,
			}) => (Some((renditions, rendition)), Some(audio), Some(clock), Some(health)),
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
			encoding: stats.encoding.load(Ordering::Relaxed),
			watched: stats.watched.load(Ordering::Relaxed),
			encoder: stats.encoder(),
			failed: stats.error(),
			stalled,
		}
	}

	/// Draw a canvas frame per tick of the program clock, and hand the program
	/// to the encoder, stamped `epoch` past `start`: every frame, or only while
	/// anyone watches when the output is on demand.
	///
	/// A new canvas size or frame rate ends the rendition of `video` it was
	/// drawn at and opens another, since a track never changes what it
	/// carries. Drawing outlives the encoder: a failed output stops copying the
	/// program, and the preview and program keep moving.
	async fn run_program(&self, start: Instant, epoch: Duration, video: Option<(Renditions, Rendition)>) {
		let (renditions, mut rendition) = video.unzip();
		let mut canvas = self.state.lock().unwrap().compositor.canvas();
		let mut clock = program_clock(start, canvas);
		let on_demand = self.output.as_ref().is_some_and(|output| output.on_demand);
		// One probe at a time, with the canvas it is for, since a GPU caps its
		// encode sessions.
		let mut probing: Option<(Canvas, std::thread::JoinHandle<anyhow::Result<Probe>>)> = None;
		// The encode threads of renditions ended or failed since, whose
		// sessions the next probe waits to be closed.
		let mut ending = Vec::new();
		// Whether the canvas drawn has its rendition, or failed to open one, so
		// a failure is not retried until the canvas changes.
		let mut settled = true;
		if let Some(rendition) = &rendition {
			rendition.adopted(&self.stats);
		}

		loop {
			let tick = clock.tick().await;
			let (program, drawn) = {
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
				(state.compositor.program().clone(), state.compositor.canvas())
			};
			self.stats.rendered.fetch_add(1, Ordering::Relaxed);

			if drawn != canvas {
				if drawn.framerate != canvas.framerate {
					clock = program_clock(tick + period(drawn), drawn);
				}
				canvas = drawn;
				settled = false;
				// Ended before the next opens, so the two never hold encode
				// sessions at once.
				ending.extend(rendition.take().map(Rendition::end));
				self.stats.retire_encoder();
			}
			if let Some(renditions) = &renditions {
				// Probed beside the program clock, so drawing carries on.
				if let Some((probed, probe)) = probing.take_if(|(_, probe)| probe.is_finished()) {
					let probe = probe
						.join()
						.unwrap_or_else(|_| Err(anyhow::anyhow!("the encoder probe panicked")));
					// One for a canvas since left is dropped unlisted.
					if probed == canvas {
						settled = true;
						match probe.and_then(|probe| renditions.start(probe)) {
							Ok((started, _)) => {
								started.adopted(&self.stats);
								rendition = Some(started);
							}
							Err(err) => self.stats.fail_video(format!("{err:#}")),
						}
					}
				}
				if probing.is_none() && !settled {
					let ending = std::mem::take(&mut ending);
					match Probe::spawn(renditions.gpu.clone(), canvas, ending) {
						Ok(probe) => probing = Some((canvas, probe)),
						Err(err) => {
							settled = true;
							self.stats.fail_video(format!("cannot start an encoder probe: {err}"));
						}
					}
				}
			}

			let watched = rendition
				.as_ref()
				.is_some_and(|rendition| rendition.watched.load(Ordering::Relaxed));
			self.stats.watched.store(watched, Ordering::Relaxed);
			let wanted = rendition.is_some() && (!on_demand || watched);
			self.stats.encoding.store(wanted, Ordering::Relaxed);
			let Some(current) = &mut rendition else { continue };
			let was = std::mem::replace(&mut current.encoding, wanted);
			if !wanted {
				// Behind every frame already sent, so the break lands after them.
				// A send fails only once the encoder stopped, having reported why.
				if was && current.frames.send(Video::Pause).is_err() {
					ending.extend(rendition.take().map(Rendition::end));
				}
				continue;
			}
			let Ok(timestamp) = moq_net::Timestamp::try_from(epoch + tick.saturating_duration_since(start)) else {
				continue;
			};
			match current.exporter.copy(&program, timestamp) {
				// The pool bounds the frames in flight, so this waits only behind
				// a pause the encoder has not reached yet.
				Ok(Some(pending)) => {
					if current.frames.send(Video::Frame(pending)).is_err() {
						ending.extend(rendition.take().map(Rendition::end));
					}
				}
				Ok(None) => {
					self.stats.dropped.fetch_add(1, Ordering::Relaxed);
				}
				// The canvas and the exporter share a size and format, so this is
				// the GPU failing: the video stops, and the canvas keeps drawing.
				Err(err) => {
					let _ = current.frames.send(Video::Failed(err));
					ending.extend(rendition.take().map(Rendition::end));
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
				self.stats.fail_audio(err);
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
	/// Where the program video is published, a rendition at a time.
	renditions: Renditions,
	/// The rendition of the canvas the studio started with.
	rendition: Rendition,
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
	stats: &Arc<Stats>,
	relays: Vec<Relay>,
	subscribe: moq_net::origin::Producer,
	path: &str,
	canvas: Canvas,
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

	// Probed now, so a machine that cannot encode the canvas fails at startup
	// rather than once the program clock starts.
	let probe = Probe::new(gpu, canvas).await?;
	// Sources take the program's tier: hardware when the program found it, so a
	// session the GPU refuses fails that source loudly instead of falling to
	// software. When the program could not use hardware, such as NVENC beside an
	// integrated GPU it cannot read, a source may still reach it from the CPU.
	let kind = match &probe.config.kind {
		moq_video::encode::Kind::Named(name) if name == "openh264" => moq_video::encode::Kind::Auto,
		_ => moq_video::encode::Kind::Hardware,
	};
	let sources = moq_scene::publish::Publish::new(broadcast.clone(), catalog.clone(), bandwidth, kind);
	// Held until the video and audio are listed, so the first catalog carries
	// both and the solicitation.
	let reserved = catalog.reserve();
	let mut soliciting = catalog.clone();
	let mut section = soliciting.modify()?;
	section.echo = Some(hang::catalog::Echo::new(echo));
	section.commit()?;
	let renditions = Renditions {
		gpu: gpu.clone(),
		broadcast: broadcast.clone(),
		timebase: catalog.timebase(),
		stats: stats.clone(),
		runtime: tokio::runtime::Handle::current(),
	};
	let (rendition, opened) = renditions.start(probe)?;
	// Before the program clock starts the sources, so the program holds its
	// encode session even while unwatched, and a source past the GPU's session
	// cap is the one that fails.
	opened.await.context("the encode thread stopped")??;

	let bus = moq_audio::bus::Config::default();
	let mut input = moq_audio::encode::Input::new(bus.sample_rate, bus.layout);
	input.format = moq_audio::Format::F32;
	let mut options = moq_audio::encode::Options::default();
	options.track = Some("audio".into());
	options.config = moq_audio::encode::Config::from_input(moq_audio::encode::Codec::Opus, &input);
	options.config.bitrate = Some(AUDIO_BITRATE);
	let graded = catalog.clone();
	let audio = moq_audio::encode::Producer::new(&mut broadcast, catalog, input, &options)?;
	drop(reserved);

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
		catalog: graded,
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
			renditions,
			rendition,
			audio,
			clock,
			health,
			sources,
		},
	))
}

/// Where a canvas's program video is published: one rendition at a time,
/// each its own track, since a track never changes what it carries.
#[derive(Clone)]
struct Renditions {
	gpu: Gpu,
	broadcast: moq_net::broadcast::Producer,
	/// Shared by every rendition, so each lands on the catalog's clock the same
	/// way and a switch keeps the video in step with the audio.
	timebase: moq_mux::catalog::Timebase<moq_scene::publish::Ext>,
	stats: Arc<Stats>,
	/// Where each rendition's viewers are followed.
	runtime: tokio::runtime::Handle,
}

impl Renditions {
	/// List `probe`'s rendition in the catalog under a new track, and start its
	/// encode thread, which reports on the receiver once its encoder opens.
	fn start(&self, probe: Probe) -> anyhow::Result<(Rendition, tokio::sync::oneshot::Receiver<anyhow::Result<()>>)> {
		let Probe {
			exporter,
			config,
			video,
		} = probe;
		// Before its encode thread starts, which reports its own failure.
		*self.stats.video_error.lock().unwrap() = None;
		let reserved = self
			.timebase
			.reserve()
			.with_hls_hint(config.gop.duration(config.framerate));
		let producer = moq_video::encode::Producer::new(self.broadcast.clone(), reserved, video)?;
		let watched = Arc::new(AtomicBool::new(false));
		self.runtime.spawn(watch(producer.demand(), watched.clone()));

		// One more than the exporter's pool, for a pause behind a full pool.
		let (frames, pending) = mpsc::sync_channel(Exporter::DEPTH + 1);
		let zero_copy = exporter.route() == Route::Gpu;
		let stats = self.stats.clone();
		let (opened, open) = tokio::sync::oneshot::channel();
		let number = stats.rendition();
		let thread = std::thread::Builder::new()
			.name("encode".into())
			.spawn(move || encode(&stats, number, opened, pending, producer, config, zero_copy))?;
		let rendition = Rendition {
			exporter,
			frames,
			watched,
			encoding: false,
			thread,
		};
		Ok((rendition, open))
	}
}

/// An encoder proven to take this GPU's export of a canvas, before its
/// rendition is listed.
struct Probe {
	exporter: Exporter,
	config: moq_video::encode::Config,
	video: hang::catalog::VideoConfig,
}

impl Probe {
	/// Open a throwaway encoder of `canvas` on what `gpu` exports, which also
	/// settles whether the encoder reads the program on the GPU.
	async fn new(gpu: &Gpu, canvas: Canvas) -> anyhow::Result<Self> {
		let mut config = moq_video::encode::Config::new(canvas.width, canvas.height, canvas.framerate());
		let (exporter, video) = Exporter::new(&gpu.device, &gpu.queue, &mut config)
			.await
			.with_context(|| format!("cannot encode a {}x{} canvas", canvas.width, canvas.height))?;
		Ok(Self {
			exporter,
			config,
			video,
		})
	}

	/// [`new`](Self::new) on a thread of its own, off the program clock, once
	/// the encode threads `ending` have closed their sessions.
	fn spawn(
		gpu: Gpu,
		canvas: Canvas,
		ending: Vec<std::thread::JoinHandle<()>>,
	) -> std::io::Result<std::thread::JoinHandle<anyhow::Result<Self>>> {
		std::thread::Builder::new().name("probe".into()).spawn(move || {
			for thread in ending {
				// One that panicked has closed its session all the same.
				let _ = thread.join();
			}
			let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
			runtime.block_on(Self::new(&gpu, canvas))
		})
	}
}

/// One rendition of the program video: the exporter the program clock copies
/// into, and the encode thread behind it.
struct Rendition {
	exporter: Exporter,
	frames: mpsc::SyncSender<Video>,
	/// Whether anyone subscribes to the rendition's track.
	watched: Arc<AtomicBool>,
	/// Whether the program clock is handing it frames.
	encoding: bool,
	thread: std::thread::JoinHandle<()>,
}

impl Rendition {
	/// Report this rendition as the program's from now on.
	fn adopted(&self, stats: &Stats) {
		let zero_copy = self.exporter.route() == Route::Gpu;
		stats.zero_copy.store(zero_copy, Ordering::Relaxed);
	}

	/// Stop handing it frames, so its encode thread encodes those it holds,
	/// finishes its track, and returns.
	fn end(self) -> std::thread::JoinHandle<()> {
		self.thread
	}
}

/// How often the program is drawn at `canvas`'s frame rate, from `start`.
fn program_clock(start: Instant, canvas: Canvas) -> kio::clock::Interval {
	let mut clock = kio::clock::interval_at(start, period(canvas));
	clock.set_missed_tick_behavior(kio::clock::MissedTickBehavior::Skip);
	clock
}

fn period(canvas: Canvas) -> Duration {
	Duration::from_secs(1) / canvas.framerate
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

/// Follow whether anyone watches a rendition's track until the track ends,
/// so an on-demand program is only copied and encoded while someone does.
/// The track ends only through its encode thread, which reports why.
async fn watch(mut demand: moq_net::track::Demand, watched: Arc<AtomicBool>) {
	while let Ok(wanted) = demand.wanted_changed().await {
		watched.store(wanted.is_some(), Ordering::Relaxed);
	}
	watched.store(false, Ordering::Relaxed);
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

/// Open a rendition's encoder, reporting how that went on `opened`, then
/// encode program frames until the program clock ends the rendition, and
/// finish its track.
///
/// A failure aborts the video track, so viewers see that it failed rather
/// than a track that went away unfinished.
fn encode(
	stats: &Stats,
	rendition: u64,
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
			let err = anyhow::Error::new(err).context("video encoder");
			stats.fail_video(format!("{err:#}"));
			let _ = opened.send(Err(err));
			return;
		}
	};
	stats.opened_encoder(rendition, encoder.name());
	let _ = opened.send(Ok(()));

	let result = encode_frames(stats, video, &mut producer, &mut encoder, zero_copy).and_then(|()| {
		producer.finish()?;
		Ok(())
	});
	if let Err(err) = result {
		stats.fail_video(format!("{err:#}"));
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

	/// A rendition a resize retired while its encoder was still opening must
	/// not report that encoder over the next rendition's.
	#[test]
	fn a_retired_rendition_reports_no_encoder() {
		let stats = Stats::default();
		let retired = stats.rendition();
		stats.retire_encoder();
		let next = stats.rendition();
		stats.opened_encoder(retired, "old");
		assert_eq!(stats.encoder(), None, "the retired encoder opened after the resize");
		stats.opened_encoder(next, "new");
		assert_eq!(stats.encoder().as_deref(), Some("new"));
	}

	async fn subscribe(
		broadcast: &moq_net::broadcast::Consumer,
		name: &str,
		config: &hang::catalog::VideoConfig,
	) -> moq_video::decode::Consumer {
		let mut options = moq_video::decode::Options::new();
		options.start = moq_video::decode::Start::Latest;
		moq_video::decode::Consumer::new(broadcast, config, name.to_string(), options)
			.await
			.expect("video decoder")
	}

	/// Wait for `pictures` to show the red canvas at `size`, returning when it
	/// is shown. The canvas is black until the color source delivers its first
	/// picture, so this waits for the composed one rather than take the first.
	async fn red(
		pictures: &mut moq_video::decode::Consumer,
		size: moq_video::Size,
	) -> Result<moq_net::Timestamp, kio::clock::Elapsed> {
		kio::clock::timeout(TIMEOUT, async {
			loop {
				let frame = match pictures.read().await.expect("decode") {
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
					return frame.timestamp;
				}
			}
		})
		.await
	}

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
			engine.stats.encoder().is_some(),
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
		let mut pictures = subscribe(&broadcast, video, config).await;
		let shown = red(&mut pictures, moq_video::Size::new(320, 180))
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

		// A resize while live ends the old track and lists a rendition of the
		// new size and frame rate under a new one, on the same timeline.
		{
			let mut state = engine.state.lock().unwrap();
			let mut draft = state.collection().draft("Resize the canvas");
			let canvas = moq_scene::Canvas {
				width: 160,
				height: 90,
				framerate: 15,
			};
			draft.set_canvas(canvas).unwrap();
			state.commit(draft, None).unwrap();
		}
		let ended = kio::clock::timeout(TIMEOUT, async {
			let mut last = shown;
			while let Some(read) = pictures.read().await.expect("decode") {
				if let moq_video::decode::Read::Frame(frame) = read {
					last = frame.timestamp;
				}
			}
			last
		})
		.await
		.expect("the old track ends");
		let (resized, config) = kio::clock::timeout(TIMEOUT, async {
			loop {
				let catalog = catalogs.next().await.expect("catalog").expect("a catalog");
				if let [(name, config)] = catalog.video.enabled().collect::<Vec<_>>().as_slice()
					&& *name != video
				{
					return ((*name).clone(), (*config).clone());
				}
			}
		})
		.await
		.expect("the catalog lists the new rendition alone");
		let mut pictures = subscribe(&broadcast, &resized, &config).await;
		let first = red(&mut pictures, moq_video::Size::new(160, 90))
			.await
			.expect("the resized picture arrives");
		assert!(ended < first, "the new rendition continues the old one's timeline");
		let next = kio::clock::timeout(TIMEOUT, async {
			loop {
				match pictures.read().await.expect("decode") {
					Some(moq_video::decode::Read::Frame(frame)) => return frame.timestamp,
					Some(_) => continue,
					None => panic!("the video ended"),
				}
			}
		})
		.await
		.expect("a second resized picture arrives");
		// At least 1/15 s apart, as the old 30 fps clock would not be.
		let gap = Duration::from(next) - Duration::from(first);
		assert!(
			gap > Duration::from_millis(50),
			"the program clock follows the frame rate: {gap:?}"
		);

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
