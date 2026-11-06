//! Everything behind the window: the GPU, the sources, the program clock that
//! draws the canvas and mixes the audio, and the broadcast it encodes into:
//! the program, at the canvas size and at half of it ([`crate::ladder`]). The
//! relays serve that broadcast, and recordings read it. Each local source's own
//! track stays in a broadcast of the studio's own, which recordings read too.
//!
//! Threads:
//! - **program** draws preview and program once per canvas frame and hands the
//!   program to each rendition's encoder its gate lets encode, scaled to the
//!   rendition and stamped with the program clock.
//! - **audio** advances the audio bus, and with it every mix, on the same
//!   clock.
//! - **audio-encode** encodes the mix on program into the program audio, and
//!   **audio-record** the mix recordings hold into the recording broadcast's
//!   own audio while a recording reads it.
//! - Each guest's return feed encodes its mix-minus on the tokio runtime.
//! - **encode**: one per program rendition, which waits for each program copy
//!   and encodes it. A new canvas size or frame rate starts new renditions.
//! - Recordings write their files on the tokio runtime.
//! - **source-***: one per running source, since a capture stream may not
//!   leave the thread that opened it.
//! - **autosave** writes the collection and its history to their files once
//!   edits to them settle, and takes or holds an outside edit to the scene file.
//! - The UI runs on the main thread and edits [`State`] under its lock.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::Context;
use bytes::Bytes;
use kio::clock::Instant;
use moq_net::bandwidth;
use moq_scene::publish::{Encoding, Sessions};
use moq_scene::source::{self, Driver, Sources, Status};
use moq_scene::{Canvas, Collection, Compositor, Draft, Edit, History, Scaler, wgpu};
use moq_video::render::export::{Exporter, Pending, Route};
use tokio::sync::oneshot::error::TryRecvError;

use crate::autosave::{self, Autosave};
use crate::ladder::{self, Gate, Joins, Step};
use crate::{health, mix, record};

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
	/// How many times the collection was reloaded from its files, so the UI
	/// can let go of what a reload may have removed.
	pub reloads: u64,
	pub sources: Sources,
	pub compositor: Compositor,
	/// Edited since the sources were last synced.
	stale: bool,
	/// For the autosave, which writes the collection once edits settle.
	edits: autosave::Edits,
	/// What became of each scene the edits, undos, and redos since the window
	/// last took them renamed or removed, one map per edit as
	/// [`Collection::renames`] reads it, so what the window follows a scene by
	/// name with, such as a hotkey, follows it too.
	renamed: Vec<BTreeMap<String, Option<String>>>,
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

	/// Apply `draft`, a change the user did not make, such as a guest joining,
	/// without recording it: it is not undone, and drops nothing that could be
	/// redone. See [`History::apply_outside`] for what it may change.
	pub fn apply_outside(&mut self, draft: Draft) -> anyhow::Result<()> {
		let edit = draft.finish()?;
		self.history.apply_outside(&mut self.collection, &edit)?;
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
	/// `collection` lacks is dropped. Edits that remove a scene on air are
	/// refused, changing nothing.
	fn adopt(&mut self, collection: Collection, made: &[Edit]) -> anyhow::Result<()> {
		let mut before = self.collection.clone();
		let mut renames = Vec::with_capacity(made.len());
		for edit in made {
			renames.push(before.renames(edit));
			before.apply(edit)?;
		}
		keep_on_air(self.compositor.on_air(), &renames)?;
		self.compositor.set_canvases(&collection)?;
		for renames in &renames {
			self.compositor.rename(renames);
		}
		self.compositor.retain(&collection);
		self.collection = collection;
		self.renamed
			.extend(renames.into_iter().filter(|renames| !renames.is_empty()));
		Ok(())
	}

	/// The scene renames and removals since the last call, oldest first.
	pub fn take_renames(&mut self) -> Vec<BTreeMap<String, Option<String>>> {
		std::mem::take(&mut self.renamed)
	}

	/// The sources whose pictures any canvas's program draws, through clones:
	/// those [`placed`](Self::placed) there, and the clip a stinger under way
	/// plays with each mask its source's filters read.
	fn shown(&self) -> BTreeSet<String> {
		let collection = &self.collection;
		let stinger = self.compositor.stinger().into_iter().flat_map(|clip| {
			let filters = collection.video.get(clip).into_iter().flat_map(|video| &video.filters);
			std::iter::once(clip).chain(masks(filters).map(String::as_str))
		});
		let mut shown = self.placed();
		shown.extend(stinger.map(|source| collection.capture(source).to_string()));
		shown
	}

	/// The sources whose pictures the scenes on air place on any canvas's
	/// program, through clones: each visible item's of the scenes on air,
	/// including one leaving in a transition, and of the top layers over them,
	/// and each mask's it reads.
	fn placed(&self) -> BTreeSet<String> {
		let collection = &self.collection;
		let scenes: Vec<Option<&str>> = self.compositor.on_air().map(Some).chain([None]).collect();
		let canvases = std::iter::once(None).chain(collection.canvases.keys().map(Some));
		let items = canvases
			.flat_map(|canvas| {
				scenes.iter().map(move |scene| moq_scene::Layer {
					canvas: canvas.cloned(),
					scene: scene.map(str::to_string),
				})
			})
			.flat_map(|layer| collection.items(&layer).unwrap_or_default())
			.filter(|item| item.visible);
		items
			.flat_map(|item| std::iter::once(&item.source).chain(masks(collection.filters(item))))
			.map(|source| collection.capture(source).to_string())
			.collect()
	}

	/// Restart the source called `name`, as an edit would, such as to reload a
	/// web page.
	pub fn restart(&mut self, name: &str) {
		self.sources.restart(name);
		self.stale = true;
	}

	/// After any change to the collection or its history, which the autosave
	/// writes once they settle.
	fn edited(&mut self) {
		self.stale = true;
		self.edits.record();
	}
}

/// The sources the masks among `filters` read: a mask's picture shapes what
/// the program draws too.
fn masks<'a>(filters: impl Iterator<Item = &'a moq_scene::Filter>) -> impl Iterator<Item = &'a String> {
	filters.filter_map(|filter| match filter {
		moq_scene::Filter::Mask { source, .. } => Some(source),
		_ => None,
	})
}

impl autosave::Document for State {
	/// Both under one lock, so the history describes the collection beside it.
	fn text(&self) -> anyhow::Result<autosave::Text> {
		autosave::Text::new(&self.collection, &self.history)
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
		self.reloads += 1;
		self.stale = true;
		Ok(())
	}
}

/// Refuse edits, each read as its [`Collection::renames`], that remove a
/// scene `on_air` program draws, so a slip cannot put black on air. A reload
/// that leaves one out still drops it, having nothing to show instead.
fn keep_on_air<'a>(
	on_air: impl IntoIterator<Item = &'a str>,
	renames: &'a [BTreeMap<String, Option<String>>],
) -> anyhow::Result<()> {
	for mut scene in on_air {
		for renames in renames {
			match renames.get(scene) {
				Some(Some(to)) => scene = to,
				Some(None) => anyhow::bail!("`{scene}` is on air; cut to another scene before removing it"),
				None => {}
			}
		}
	}
	Ok(())
}

/// How the output is doing, for the stream panel.
#[derive(Default)]
pub struct Stats {
	/// Canvas frames drawn.
	pub rendered: AtomicU64,
	/// The main canvas's program video.
	pub video: Arc<VideoStats>,
	/// Each further canvas's program video, by the canvas's name, while it is
	/// drawn.
	canvases: Mutex<BTreeMap<String, Arc<VideoStats>>>,
	/// The video encoders open: every canvas's program and the sources'.
	pub sessions: Sessions,
	/// Each program track's share of the connection's bandwidth, by name: a
	/// further canvas's prefixed with its own.
	grants: Mutex<BTreeMap<String, bandwidth::Consumer>>,
	/// Why the audio stopped, if it did.
	audio_error: Mutex<Option<String>>,
	/// Why the recording's audio stopped, if it did, which leaves the published
	/// audio running.
	recording_audio_error: Mutex<Option<String>>,
}

/// How one canvas's program video is doing.
#[derive(Default)]
pub struct VideoStats {
	/// What each of its video encoders costs: the canvas size, the half size,
	/// and, for the main canvas, the recording's own canvas size.
	pub renditions: [Load; 3],
	/// Whether the encoder reads the program on the GPU, rather than each frame
	/// being read back to the CPU.
	pub zero_copy: AtomicBool,
	/// Whether anyone subscribes to any of the video's renditions: a viewer
	/// through the relay, or a recording.
	pub watched: AtomicBool,
	/// Whether the program is handed to the canvas size's encoder now: not
	/// while on demand and unwatched, nor while a new rendition opens, nor
	/// while the grant cannot fund it.
	pub encoding: AtomicBool,
	/// How many sizes the video has now: the canvas size, and the half size
	/// unless the canvas is too small or its encoder refused it.
	pub sizes: std::sync::atomic::AtomicUsize,
	/// The encoder backend, once the current rendition opened one.
	encoder: Mutex<EncoderName>,
	/// Why the video stopped, if it did, until a new rendition starts.
	error: Mutex<Option<String>>,
}

/// What one of the program's renditions costs.
#[derive(Default)]
pub struct Load {
	/// Program frames handed to its encoder.
	pub copied: AtomicU64,
	/// Microseconds the last one waited on the GPU, and on a readback converted
	/// pixels.
	pub copy_us: AtomicU64,
	/// Program frames read back to the CPU for its encoder.
	pub read_back: AtomicU64,
	/// Program frames not encoded because its encoder still held every copy.
	pub dropped: AtomicU64,
	/// Program frames encoded.
	pub encoded: AtomicU64,
	/// Microseconds the last encode took.
	pub encode_us: AtomicU64,
	/// Why this encoder stopped, or could not start, while the canvas size
	/// carries on, until the next canvas. The canvas size's own failure stops
	/// the video, which [`VideoStats::error`] reports.
	error: Mutex<Option<String>>,
}

impl Load {
	/// Why this encoder stopped, if it did; see [`Load::error`].
	pub fn error(&self) -> Option<String> {
		self.error.lock().unwrap().clone()
	}

	fn fail(&self, err: impl std::fmt::Display) {
		tracing::warn!(%err, "a program rendition stopped");
		*self.error.lock().unwrap() = Some(err.to_string());
	}
}

/// The encoder backend the current rendition opened, if it has.
#[derive(Default)]
struct EncoderName {
	/// Counts the renditions, so one a resize retired while its encoder was
	/// still opening cannot report that encoder afterwards.
	rendition: u64,
	name: Option<String>,
}

impl VideoStats {
	/// The encoder backend, once the current rendition opened one.
	pub fn encoder(&self) -> Option<String> {
		self.encoder.lock().unwrap().name.clone()
	}

	/// The number of the current rendition, which its encode thread reports
	/// its encoder under.
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

	/// Report `name` as the encoder, unless `rendition` was retired.
	fn opened_encoder(&self, rendition: u64, name: &str) {
		let mut encoder = self.encoder.lock().unwrap();
		if encoder.rendition == rendition {
			encoder.name = Some(name.to_string());
		}
	}

	/// Why the video stopped, if it did.
	pub fn error(&self) -> Option<String> {
		self.error.lock().unwrap().clone()
	}

	fn fail(&self, err: impl std::fmt::Display) {
		tracing::error!(%err, "video stopped");
		*self.error.lock().unwrap() = Some(format!("video: {err}"));
	}
}

impl Stats {
	/// Each further canvas's program video, by the canvas's name.
	pub fn canvases(&self) -> BTreeMap<String, Arc<VideoStats>> {
		self.canvases.lock().unwrap().clone()
	}

	/// Each program track's share of the connection's bandwidth now, by name:
	/// `None` while the connection has no estimate.
	pub fn grants(&self) -> BTreeMap<String, Option<bandwidth::Rate>> {
		let grants = self.grants.lock().unwrap();
		grants
			.iter()
			.map(|(name, grant)| (name.clone(), grant.peek()))
			.collect()
	}

	/// Why the main canvas's output stopped, if it did.
	pub fn error(&self) -> Option<String> {
		let video = self.video.error();
		let audio = self.audio_error.lock().unwrap().clone();
		match (video, audio) {
			(Some(video), Some(audio)) => Some(format!("{video}; {audio}")),
			(video, audio) => video.or(audio),
		}
	}

	/// The recording's audio encoder failed, which stops what recordings hold but
	/// not the published program.
	fn fail_recording_audio(&self, err: impl std::fmt::Display) {
		tracing::error!(%err, "the recording's audio stopped");
		*self.recording_audio_error.lock().unwrap() = Some(format!("{err}"));
	}

	/// Why the recording's audio stopped, if it did.
	pub fn recording_audio_error(&self) -> Option<String> {
		self.recording_audio_error.lock().unwrap().clone()
	}

	fn fail_audio(&self, err: impl std::fmt::Display) {
		tracing::error!(%err, "audio stopped");
		*self.audio_error.lock().unwrap() = Some(format!("audio: {err}"));
	}
}

/// The relays the program goes live on.
pub struct Output {
	/// The broadcast path on the relays.
	pub path: String,
	/// Whether the program video's canvas size is copied and encoded only
	/// while someone watches it, rather than all the time. The half size
	/// always is.
	pub on_demand: bool,
	/// A connection to each relay, which reconnects on its own and dials only
	/// while held. The Health panel reads their states.
	connections: Vec<moq_tokio::Connection>,
	/// The client that dialed each, whose endpoint sends the last close.
	clients: Vec<moq_tokio::Client>,
}

impl Output {
	/// Close each relay session once the relay has received what it was sent
	/// of every finished track, which the session bounds at about a second,
	/// then each client's endpoint, so the relay hears the close rather than
	/// waiting out an idle timeout. A track still live is cut.
	async fn close(&self) {
		let mut closing = tokio::task::JoinSet::new();
		for (connection, client) in self.connections.iter().zip(&self.clients) {
			let (connection, client) = (connection.clone(), client.clone());
			closing.spawn(async move {
				if let Err(err) = connection.close().await {
					tracing::warn!(%err, "the relay session closed without draining");
				}
				client.close().await;
			});
		}
		closing.join_all().await;
	}
}

/// A relay the program is published to.
#[derive(Clone)]
pub struct Relay {
	pub client: moq_tokio::Client,
	pub url: url::Url,
}

/// The program's broadcast, open for as long as the engine runs.
pub struct Program {
	/// Where the broadcast is announced, which the relay connections publish.
	_origin: moq_net::origin::Producer,
	publication: moq_net::origin::Publication,
	/// What the broadcast lists and the clock its timestamps are on, for a
	/// replay of the program or a source.
	catalog: moq_scene::publish::Catalog,
	clock: moq_mux::Clock,
	/// Where the local sources keep their own tracks, on the same clock.
	sources: moq_scene::publish::Publish,
	/// What a recording reads the program from, on the same clock.
	recording: moq_net::origin::Publication,
	/// The recording broadcast's catalog, held for as long as the engine runs,
	/// so a recording still reading as the studio stops finds it open.
	_recorded: moq_scene::publish::Catalog,
	/// The origin the studio's own broadcasts are read through, which no relay
	/// connection carries.
	local: moq_net::origin::Producer,
	/// The audio's claim on the connection's bandwidth, which it does not
	/// follow: Opus at this rate is never what congests the uplink.
	_audio: bandwidth::Reservation,
}

impl Drop for Program {
	fn drop(&mut self) {
		self.publication.close();
	}
}

impl Program {
	/// What a replay of the program could copy: its video at each size,
	/// largest first, with its audio, up to now.
	pub fn program(&self) -> Vec<moq_scene::replay::Target> {
		let catalog = self.catalog.snapshot();
		let audio = catalog.audio.renditions.into_iter().next();
		let mut video: Vec<_> = catalog.video.renditions.into_iter().collect();
		video.sort_by_key(|(_, config)| {
			let pixels = u64::from(config.coded_width.unwrap_or(0)) * u64::from(config.coded_height.unwrap_or(0));
			std::cmp::Reverse(pixels)
		});
		match video.is_empty() {
			true => vec![self.target(None, audio)],
			false => video
				.into_iter()
				.map(|video| self.target(Some(video), audio.clone()))
				.collect(),
		}
	}

	/// What a replay of the local source `name` copies: the video or sound of
	/// its own track, up to now, or `None` while it publishes neither.
	pub fn source(&self, name: &str) -> Option<moq_scene::replay::Target> {
		let mut catalog = self.sources.catalog();
		let listed = catalog.ext.studio.sources.remove(name)?;
		let video = listed.video.renditions.into_iter().next();
		let audio = listed.audio.renditions.into_iter().next();
		if video.is_none() && audio.is_none() {
			return None;
		}
		Some(moq_scene::replay::Target {
			broadcast: self.sources.consume(),
			video,
			audio,
			end: self.clock.now(),
		})
	}

	fn target(
		&self,
		video: Option<(String, hang::catalog::VideoConfig)>,
		audio: Option<(String, hang::catalog::AudioConfig)>,
	) -> moq_scene::replay::Target {
		moq_scene::replay::Target {
			broadcast: self.publication.broadcast().consume(),
			video,
			audio,
			end: self.clock.now(),
		}
	}

	/// Encode the local sources' tracks on program, whether or not anything
	/// reads them, so a replay of one has its past: the pictures `shown` in a
	/// scene on air on any canvas, and the sound the program mix plays. A media file heard
	/// and not shown encodes only its sound.
	fn on_program(&self, state: &State, shown: BTreeSet<String>) {
		let program = &state.collection.mixes.program;
		let heard = state
			.sources
			.iter()
			.filter(|(name, _)| state.sources.plays(program, name))
			.map(|(name, _)| name.to_string())
			.collect();
		self.sources
			.set_on_program(moq_scene::publish::OnProgram { shown, heard });
	}
}

/// The running studio.
pub struct Engine {
	pub state: Mutex<State>,
	pub autosave: Autosave,
	pub stats: Arc<Stats>,
	/// How the stream is delivered, graded every [`health::INTERVAL`].
	pub health: Mutex<health::Report>,
	/// The mix the program audio carries, as the collection picks it.
	program_audio: mix::Pick,
	/// The mix recordings hold, as the collection picks it.
	recorded_audio: mix::Pick,
	/// Plays a mix to the speakers while the creator listens.
	monitor: Mutex<Option<mix::Monitor>>,
	/// `None` when running without a relay.
	pub output: Option<Output>,
	/// Records the program, and each source too if asked.
	pub recorder: record::Recorder,
	/// The room guests join, when configured.
	pub room: Option<Arc<crate::guests::Room>>,
	/// Each guest's return feed, once the studio's session in the room dials.
	feeds: Mutex<Option<crate::feed::Feeds>>,
	/// The program's broadcast.
	pub program: Program,
	/// Keeps broadcast sources' origin open when no connection holds it.
	_origin: Option<moq_net::origin::Producer>,
	/// Set by [`stop`](Self::stop), which ends the program clock.
	stopped: AtomicBool,
	/// The program clock's and the audio's threads, which [`stop`](Self::stop)
	/// waits on.
	threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
	/// Each source thread's end, which [`close`](Self::close) waits on.
	running: Mutex<Vec<tokio::sync::oneshot::Receiver<()>>>,
}

/// The audio the program publishes.
pub const AUDIO_BITRATE: moq_net::bandwidth::Rate = moq_net::bandwidth::Rate::from_kbps(128);

/// How often the audio thread advances the bus. Short enough that a block is never
/// close to the bus's buffering, long enough to cost nothing.
const AUDIO_PERIOD: Duration = Duration::from_millis(10);

/// The longest [`Engine::close`] waits for the sources to stop and the relays
/// to drain, so a source or relay that never answers cannot hold up quitting.
/// Each relay session bounds its own drain at about a second.
const CLOSE: Duration = Duration::from_secs(3);

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
	/// The directory recordings are saved in.
	pub recordings: PathBuf,
	/// The room guests join, if any.
	pub room: Option<Arc<crate::guests::Room>>,
}

impl Engine {
	/// Open the `loaded` collection and history on `launch.gpu`, start the
	/// program clock, and save both to `autosave`'s files as they change,
	/// starting at once.
	/// The program is encoded into a broadcast at `launch.broadcast` while
	/// anyone wants it: a recording into `launch.recordings`, or every one of
	/// `launch.relays`, preferred first, which want it all the time unless
	/// `launch.on_demand`. The guests who join `launch.room` are added as
	/// sources.
	///
	/// Call inside the tokio runtime that serves the relay connections.
	pub async fn start(launch: Launch, autosave: Autosave, loaded: autosave::Loaded) -> anyhow::Result<Arc<Self>> {
		let Launch {
			gpu,
			relays,
			broadcast,
			on_demand,
			recordings,
			room,
		} = launch;
		let (bus, mixer) = moq_audio::bus::Bus::new(Default::default())?;
		let subscribe = moq_tokio::origin::spawn();
		// Every relay feeds `subscribe`, so a guest either one routes is seen.
		let guests = match &room {
			Some(room) => Some(
				subscribe
					.consume()
					.scope(room.path(), &moq_auth::Pattern::all().into())
					.context("cannot follow the room")?,
			),
			None => None,
		};
		let sources = subscribe.consume();
		let autosave::Loaded {
			collection,
			history,
			dropped,
		} = loaded;
		let canvas = collection.canvas;
		let stats = Arc::new(Stats::default());

		let path =
			moq_net::Path::new(broadcast.as_str()).with_context(|| format!("cannot publish at `{broadcast}`"))?;
		let origin = moq_tokio::origin::spawn();
		let (live, subscribe) = if relays.is_empty() {
			(None, Some(subscribe))
		} else {
			tracing::info!(broadcast = %path, "publishing");
			let reports = subscribe.consume();
			let clients = relays.iter().map(|relay| relay.client.clone()).collect();
			let (connections, links) = connect(relays, &origin, &subscribe);
			let live = Live {
				connections,
				clients,
				links,
				reports,
				on_demand,
			};
			(Some(live), None)
		};
		let (program, publish) = publish(&gpu, &stats, origin, &path, canvas, live.as_ref()).await?;
		// Nothing in either local broadcast refers to another.
		let local = |broadcast: moq_net::broadcast::Consumer| moq_mux::Source::new(program.local.consume(), broadcast);
		let recorder = record::Recorder::new(
			local(program.recording.broadcast().consume()),
			local(program.sources.consume()),
			&recordings,
			autosave.path(),
		)?;
		let output = live.map(|live| Output {
			path: path.to_string(),
			on_demand,
			connections: live.connections,
			clients: live.clients,
		});

		// Absolute, so a copied image names the same file in another collection.
		let dir = autosave.path().parent().filter(|dir| !dir.as_os_str().is_empty());
		let dir = std::path::absolute(dir.unwrap_or(Path::new(".")))?;
		let sources = Sources::new(source::Context {
			bus: bus.clone(),
			origin: sources,
			dir,
			output: moq_video::render::output(&gpu.device),
			publish: Some(publish.sources.clone()),
			browser: browser(&gpu)?,
		});
		let mut compositor = Compositor::new(&gpu.device, &gpu.queue, collection.canvas)?;
		compositor.set_canvases(&collection)?;
		compositor.set_preview(collection.scenes.first().map(|scene| scene.name.clone()));
		compositor.cut(&sources);

		let engine = Arc::new(Self {
			state: Mutex::new(State {
				collection,
				history,
				dropped,
				reloads: 0,
				sources,
				compositor,
				stale: true,
				edits: autosave.edits(),
				renamed: Vec::new(),
			}),
			autosave,
			stats,
			health: Mutex::default(),
			program_audio: mix::Pick::default(),
			recorded_audio: mix::Pick::default(),
			monitor: Mutex::default(),
			output,
			recorder,
			room,
			feeds: Mutex::default(),
			program,
			_origin: subscribe,
			stopped: AtomicBool::new(false),
			threads: Mutex::default(),
			running: Mutex::default(),
		});
		let Publish {
			renditions,
			ladder,
			extras,
			audio,
			recorded_audio,
			clock,
			health,
			sources: _,
		} = publish;

		let start = Instant::now();
		// Where the program clock starts on the catalog's, so the wall time the
		// catalog advertises for a frame is when the program drew it.
		let epoch = Duration::from(clock.capture(start)?);
		if let Some(room) = &engine.room
			&& let Some(origin) = room.origin()
		{
			let program = engine.program.publication.broadcast().consume();
			let feeds = crate::feed::Feeds::new(room.clone(), origin.clone(), program, clock.clone(), epoch)?;
			*engine.feeds.lock().unwrap() = Some(feeds);
		}
		let main = Line::new(renditions, canvas, ladder, true);
		// Opened before the program clock starts the sources, as the main
		// canvas's encoder is, so each holds its encode session first.
		let mut lines = Lines::new();
		let canvases: Vec<(String, Canvas)> = {
			let state = engine.state.lock().unwrap();
			let collection = state.collection();
			let canvases = collection.canvases.keys();
			canvases
				.filter_map(|name| Some((name.clone(), collection.canvas(Some(name))?)))
				.collect()
		};
		for (name, canvas) in canvases {
			let video = Arc::new(VideoStats::default());
			engine
				.stats
				.canvases
				.lock()
				.unwrap()
				.insert(name.clone(), video.clone());
			let opened = extras.open(&name, canvas, video.clone(), &engine.program).await;
			let opened = opened.inspect_err(|err| video.fail(format!("{err:#}"))).ok();
			lines.insert(name, opened);
		}
		// Each worker holds the engine, so one that cannot start stops those
		// already running, which would otherwise keep this engine and its
		// publication alive beside a retry's.
		let starting = Starting(Some(engine.clone()));
		let program = engine.clone();
		let thread = clocked("program".into(), async move || {
			program.run_program(start, epoch, main, extras, lines).await
		})?;
		engine.threads.lock().unwrap().push(thread);
		let mix = engine.clone();
		let config = bus.config().clone();
		let rate = config.sample_rate;
		let thread = clocked("audio".into(), async move || mix.run_audio(start, config, mixer).await)?;
		engine.threads.lock().unwrap().push(thread);
		// Each on its own thread, so an encoder that falls behind skips its own
		// audio rather than holding up the bus or the other.
		let audio = [
			("audio-encode", Audio::Program(audio), engine.program_audio.follow()),
			(
				"audio-record",
				Audio::Recording(recorded_audio),
				engine.recorded_audio.follow(),
			),
		];
		for (name, audio, follow) in audio {
			let encode = engine.clone();
			let thread = clocked(name.into(), async move || {
				encode.encode_audio(epoch, rate, follow, audio).await
			})?;
			engine.threads.lock().unwrap().push(thread);
		}
		let save = engine.clone();
		clocked("autosave".into(), async move || save.autosave.run(&save.state).await)?;
		starting.started();
		tokio::spawn(engine.clone().grade(health::Sampler::new(health)));
		// Last, once nothing can fail: a start that fails is retried from setup,
		// and a follower already running would keep this engine and its
		// publication alive beside the retry's.
		if let Some(guests) = guests {
			tokio::spawn(crate::guests::run(engine.clone(), guests));
		}
		if let Some(room) = &engine.room {
			// Who has a return feed follows who is renewed, synced with the mixes.
			let mut changes = room.changes();
			// Including any made since the feeds started.
			changes.mark_changed();
			let follow = engine.clone();
			tokio::spawn(async move {
				while changes.changed().await.is_ok() {
					follow.state.lock().unwrap().stale = true;
				}
			});
		}

		Ok(engine)
	}

	/// Stop the program clock, then wait for each rendition's encoder, on
	/// every canvas, to publish the copies on their way to it and what it
	/// still holds, and to end its video track, so quitting loses none of the
	/// program's last pictures; a further canvas's catalog then finishes too.
	/// The program audio ends its track too, so a recording reading
	/// the program reaches the end of each. Then stop every source, which
	/// finishes its own track on its thread, and the browser sources' helper,
	/// waiting for it to exit, so their profile is saved and free for the next
	/// launch.
	pub fn stop(&self) {
		self.stopped.store(true, Ordering::Relaxed);
		let threads = std::mem::take(&mut *self.threads.lock().unwrap());
		for thread in threads {
			let name = thread.thread().name().unwrap_or_default().to_string();
			if thread.join().is_err() {
				tracing::error!(name, "the thread panicked");
			}
		}
		// Taken out first, so the wait holds no lock the window draws with.
		let browser = {
			let mut state = self.state.lock().unwrap();
			state.sources.stop();
			state.sources.context().browser.clone()
		};
		if let Some(browser) = browser {
			browser.stop();
		}
	}

	/// Once [`stop`](Self::stop) has ended the tracks of every canvas, close
	/// each relay session once the relay has their tails, so a viewer or a
	/// recording on the relay receives them rather than a cut. Meanwhile wait
	/// for each source's thread to end, having finished its own track. Bounded
	/// at [`CLOSE`].
	pub async fn close(&self) {
		let running = std::mem::take(&mut *self.running.lock().unwrap());
		let sources = async {
			for ended in running {
				// Fails once the thread ends, which is all it says.
				let _ = ended.await;
			}
		};
		let relays = async {
			if let Some(output) = &self.output {
				output.close().await;
			}
		};
		// A source's track is in the studio's own local broadcast, which no
		// relay session carries, so neither waits on the other.
		let closing = async { tokio::join!(sources, relays) };
		if kio::clock::timeout(CLOSE, closing).await.is_err() {
			tracing::warn!(timeout = ?CLOSE, "quitting before every source stopped and every relay drained");
		}
	}

	/// Write the collection and its history to their files unless they
	/// already hold them, or the scene file changed outside the studio.
	pub fn save(&self) -> anyhow::Result<()> {
		self.autosave.save(&self.state)
	}

	/// Replace the collection and its history with what the files hold now,
	/// dropping any unsaved edit.
	pub fn reload(&self) -> anyhow::Result<()> {
		self.autosave.reload(&self.state)
	}

	/// Write the collection and its history over what the files hold now.
	pub fn keep(&self) -> anyhow::Result<()> {
		self.autosave.keep(&self.state)
	}

	/// Play a mix to an output on `runtime`, from the next frame, or stop with
	/// `None`. A monitored mix that leaves the collection hands over to the one
	/// the program carries, and another output takes over from the old one.
	///
	/// Takes the `state` already locked, so an edit renaming the mix and the
	/// pick that follows it land before the next frame's sync.
	pub fn listen(
		&self,
		state: &mut State,
		runtime: &tokio::runtime::Handle,
		listen: Option<mix::Listen<'_>>,
	) -> anyhow::Result<()> {
		let mut monitor = self.monitor.lock().unwrap();
		match (listen, monitor.as_mut()) {
			(None, _) => *monitor = None,
			(Some(listen), Some(monitor)) if monitor.failed().is_none() => {
				monitor.set_name(listen.mix);
				monitor.set_speaker(listen.speaker);
			}
			// Listening again after a failure starts over.
			(Some(listen), _) => *monitor = Some(mix::Monitor::start(runtime, listen.mix, listen.speaker)?),
		}
		// Synced on the next frame, which picks the mix by its name.
		state.stale = true;
		Ok(())
	}

	/// The mix played to the speakers, and why it stopped if it did.
	pub fn listening(&self) -> Option<(String, Option<String>)> {
		let monitor = self.monitor.lock().unwrap();
		monitor
			.as_ref()
			.map(|monitor| (monitor.name().to_string(), monitor.failed()))
	}

	/// Point the program audio, the recorded audio, the monitor, and each
	/// guest's return feed at the mixes `state` now has.
	fn pick_mixes(&self, state: &State) {
		let mixes = &state.collection.mixes;
		if let Some(mix) = state.sources.mix(&mixes.program) {
			self.program_audio.set(mix.clone());
		}
		if let Some(mix) = state.sources.mix(mixes.recorded()) {
			self.recorded_audio.set(mix.clone());
		}
		// From the mix on program, which is what each guest follows.
		if let Some(feeds) = self.feeds.lock().unwrap().as_mut() {
			feeds.sync(&state.collection, &state.sources);
		}
		if let Some(monitor) = self.monitor.lock().unwrap().as_mut() {
			if !mixes.names.iter().any(|name| name == monitor.name()) {
				monitor.set_name(&mixes.program);
			}
			if let Some(mix) = state.sources.mix(monitor.name()) {
				monitor.play(mix.clone());
			}
		}
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
		// A stalled picture matters where any canvas's program draws it: a scene on air,
		// including one leaving in a transition, or the top layer over them.
		// Stalled sound matters only if the mix on program plays it, not one
		// only listened to. A source that stopped matters either way.
		let (stalled, stopped) = {
			let state = self.state.lock().unwrap();
			let shown = state.shown();
			let heard = |name: &str| state.sources.plays(&state.collection.mixes.program, name);
			let stalled = state
				.sources
				.iter()
				.filter_map(|(name, feed)| {
					let stall = feed.stalled(now)?;
					let matters = match stall {
						source::Stall::Picture(_) => shown.contains(name),
						source::Stall::Sound(_) => heard(name),
					};
					matters.then(|| (name.to_string(), stall))
				})
				.collect();
			// A stinger's clip rests ended between stingers, unless a scene on
			// air places it.
			let stinger = state.collection.transition.stinger();
			let placed = state.placed();
			let stopped = state
				.sources
				.iter()
				.filter(|(name, _)| shown.contains(*name) || heard(name))
				.filter_map(|(name, feed)| {
					// A picture that fails to draw fails the source as the Sources panel shows it.
					let status = match state.compositor.error(name) {
						Some(err) => Status::Failed(err.to_string()),
						None => feed.status(),
					};
					let stopped = match status {
						Status::Ended => stinger != Some(name) || placed.contains(name),
						Status::Failed(_) => true,
						_ => false,
					};
					stopped.then(|| (name.to_string(), status))
				})
				.collect();
			(stalled, stopped)
		};
		let video = &stats.video;
		let canvas = &video.renditions[0];
		health::Local {
			rendered: stats.rendered.load(Ordering::Relaxed),
			dropped: canvas.dropped.load(Ordering::Relaxed),
			encoded: canvas.encoded.load(Ordering::Relaxed),
			encoding: video.encoding.load(Ordering::Relaxed),
			grants: stats.grants(),
			watched: video.watched.load(Ordering::Relaxed),
			encoder: video.encoder(),
			failed: stats.error(),
			stalled,
			stopped,
		}
	}

	/// Draw every canvas once per tick of the program clock, and hand each
	/// canvas's program to its renditions' encoders, stamped `epoch` past
	/// `start`, while their gates let them encode: the canvas size every frame
	/// while live and not on demand, otherwise only while anyone watches it,
	/// the half size only while anyone watches it, and the main canvas's
	/// recording copy only while recording.
	///
	/// `main` encodes the main canvas, and `lines` the further canvases the
	/// studio started with. Each further canvas added since gets a [`Line`] of
	/// its own from `extras` once drawn, in a broadcast of its own, and each
	/// ends once the collection drops its canvas. Drawing outlives the encoders: a failed
	/// output stops copying its program, and the canvases keep moving.
	///
	/// Runs until the studio [stops](Self::stop), and returns once every
	/// rendition's encode thread has finished its track.
	async fn run_program(&self, start: Instant, epoch: Duration, mut main: Line, extras: Extras, mut lines: Lines) {
		let mut framerate = self.state.lock().unwrap().compositor.canvas().framerate;
		let mut clock = program_clock(start, framerate);
		// The encoders of lines since ended, waited on as the studio stops so
		// each finishes its track.
		let mut retired: Vec<Ended> = Vec::new();

		loop {
			let tick = clock.tick().await;
			if self.stopped.load(Ordering::Relaxed) {
				// Without a sender, each encoder publishes what it was sent and
				// still holds, then finishes its track.
				retired.push(main.end(None));
				retired.extend(
					lines
						.into_values()
						.flatten()
						.map(|(line, broadcast)| line.end(Some(broadcast))),
				);
				for ended in retired {
					ended.join();
				}
				return;
			}
			let drawn: Vec<(Option<String>, wgpu::Texture, Canvas)> = {
				let mut guard = self.state.lock().unwrap();
				let state = &mut *guard;
				if std::mem::take(&mut state.stale) {
					let drivers = state.sources.sync(&state.collection);
					if !drivers.is_empty() {
						let mut running = self.running.lock().unwrap();
						// Forgets the threads that have ended.
						running.retain_mut(|ended| ended.try_recv() == Err(TryRecvError::Empty));
						running.extend(drivers.into_iter().map(spawn));
					}
					self.pick_mixes(state);
				}
				// Before drawing, so a media file that restarts going on program
				// is cleared in this frame rather than showing where it ended.
				let shown = state.shown();
				state.sources.set_on_program(&shown);
				if let Err(err) = state.compositor.render(&state.collection, &state.sources, tick) {
					tracing::error!(%err, "cannot draw the canvas");
					continue;
				}
				self.program.on_program(state, shown);
				state
					.compositor
					.outputs()
					.map(|(name, output)| (name.map(str::to_string), output.program().clone(), output.canvas()))
					.collect()
			};
			self.stats.rendered.fetch_add(1, Ordering::Relaxed);

			let timestamp = moq_net::Timestamp::try_from(epoch + tick.saturating_duration_since(start)).ok();
			// Every canvas shares the main one's frame rate.
			let drawing = drawn[0].2.framerate;
			if drawing != framerate {
				framerate = drawing;
				clock = program_clock(tick + period(framerate), framerate);
			}

			retired.retain(|ended| !ended.is_finished());
			let gone: Vec<String> = lines
				.keys()
				.filter(|line| !drawn.iter().any(|(name, _, _)| name.as_ref() == Some(*line)))
				.cloned()
				.collect();
			for name in gone {
				if let Some(Some((line, broadcast))) = lines.remove(&name) {
					retired.push(line.end(Some(broadcast)));
				}
				self.stats.canvases.lock().unwrap().remove(&name);
			}

			for (name, program, canvas) in drawn {
				let line = match &name {
					None => &mut main,
					Some(name) => {
						let started = lines.entry(name.clone()).or_insert_with(|| {
							let video = Arc::new(VideoStats::default());
							self.stats.canvases.lock().unwrap().insert(name.clone(), video.clone());
							extras
								.start(name, canvas, video.clone(), &self.program)
								.inspect_err(|err| video.fail(format!("{err:#}")))
								.ok()
						});
						let Some((line, _)) = started else { continue };
						line
					}
				};
				line.feed(&program, canvas, timestamp);
			}
		}
	}

	/// Advance the bus as the program clock advances, from `start`.
	///
	/// Runs without a relay too, so the meters move and sources never back up,
	/// until the studio [stops](Self::stop), which ends every mix, so each
	/// output reads its mix to the end.
	async fn run_audio(&self, start: Instant, config: moq_audio::bus::Config, mut mixer: moq_audio::bus::Driver) {
		let rate = u128::from(config.sample_rate);
		let mut clock = kio::clock::interval_at(start, AUDIO_PERIOD);
		clock.set_missed_tick_behavior(kio::clock::MissedTickBehavior::Skip);
		let mut written: u128 = 0;

		loop {
			let tick = clock.tick().await;
			if self.stopped.load(Ordering::Relaxed) {
				drop(mixer);
				self.program_audio.close();
				self.recorded_audio.close();
				return;
			}
			// Sample-exact against the program clock, so audio and video stay
			// aligned however the ticks jitter.
			let due = tick.saturating_duration_since(start).as_nanos() * rate / 1_000_000_000;
			let frames = (due - written) as usize;
			if frames == 0 {
				continue;
			}
			mixer.advance(frames);
			written = due;
		}
	}

	/// Encode the mix `follow` reads into `audio`, stamped `epoch` past the
	/// bus's start at `rate`, until the bus stops, and finish the track.
	///
	/// The recording's audio encodes only while a recording reads it, and marks
	/// the gap as the last one stops, so the next starts on the program clock
	/// rather than where the samples left off.
	async fn encode_audio(&self, epoch: Duration, rate: u32, mut follow: mix::Follow, audio: Audio) {
		let (mut producer, recording) = match audio {
			Audio::Program(producer) => (producer, false),
			Audio::Recording(producer) => (producer, true),
		};
		// The recording's audio failing leaves the published audio running, so
		// it is the recording's to report.
		let fail = |err: moq_audio::Error| match recording {
			false => self.stats.fail_audio(err),
			true => self.stats.fail_recording_audio(err),
		};
		let rate = u128::from(rate);
		let mut next = None;
		let mut wanted = false;
		while let Some(block) = follow.read().await {
			let was = std::mem::replace(&mut wanted, !recording || producer.demand().wanted().is_some());
			if !wanted {
				if was && let Err(err) = producer.discontinuity() {
					fail(err);
					producer.abort(moq_net::StreamError::Internal.into());
					return;
				}
				next = None;
				continue;
			}
			// Audio this encoder fell too far behind to read leaves a gap in
			// the track rather than pulling what follows early.
			if next.is_some_and(|next| next != block.position) {
				producer.reanchor();
			}
			next = Some(block.position + block.frames() as u64);
			let position = u128::from(block.position);
			let micros = epoch.as_micros() + position * 1_000_000 / rate;
			let Ok(timestamp) = moq_net::Timestamp::from_micros(micros as u64) else {
				continue;
			};
			let data: Vec<u8> = block.data.iter().flat_map(|sample| sample.to_le_bytes()).collect();
			if let Err(err) = producer.write(&moq_audio::Frame::new(Bytes::from(data), timestamp)) {
				fail(err);
				// Readers see the audio fail rather than go away unfinished.
				producer.abort(moq_net::StreamError::Internal.into());
				return;
			}
		}
		if let Err(err) = producer.finish() {
			fail(err);
		}
	}
}

/// An engine whose workers are starting. Unless every one
/// [started](Self::started), dropping it stops those that did, waiting for
/// them, which takes about a frame and the encoders' tails.
struct Starting(Option<Arc<Engine>>);

impl Starting {
	fn started(mut self) {
		self.0 = None;
	}
}

impl Drop for Starting {
	fn drop(&mut self) {
		if let Some(engine) = self.0.take() {
			engine.stop();
		}
	}
}

/// One encoder of a mix: the published program audio, or the recording
/// broadcast's own.
enum Audio {
	Program(moq_audio::encode::Producer),
	Recording(moq_audio::encode::Producer),
}

/// The relay connections the program goes live on, the preferred first, and
/// the origin their viewers' reports and the relays' stats arrive on.
struct Live {
	connections: Vec<moq_tokio::Connection>,
	/// The client that dialed each connection.
	clients: Vec<moq_tokio::Client>,
	links: Vec<health::Link>,
	reports: moq_net::origin::Consumer,
	/// The canvas size encodes only while someone watches it.
	on_demand: bool,
}

/// The encoders behind the program's broadcast, handed to their threads.
struct Publish {
	/// Where the program video is published, a canvas at a time.
	renditions: Renditions,
	/// The renditions of the canvas the studio started with, unless the canvas
	/// could not be encoded without a relay.
	ladder: Option<Ladder>,
	/// What publishes each further canvas.
	extras: Extras,
	/// The program audio's encoder.
	audio: moq_audio::encode::Producer,
	/// The recording broadcast's own audio encoder.
	recorded_audio: moq_audio::encode::Producer,
	/// The catalog's clock, which the codec producers write timestamps
	/// against verbatim.
	clock: moq_mux::Clock,
	/// What the health sampler grades, while live on a relay.
	health: Option<health::Publisher>,
	/// Where each local source keeps its own track.
	sources: moq_scene::publish::Publish,
}

/// Announce the program broadcast at `path` on `origin` under one instance,
/// which `live`'s relays publish when there are any.
async fn publish(
	gpu: &Gpu,
	stats: &Arc<Stats>,
	origin: moq_net::origin::Producer,
	path: &moq_net::Path,
	canvas: Canvas,
	live: Option<&Live>,
) -> anyhow::Result<(Program, Publish)> {
	let publication = origin.create_broadcast(path).context("cannot create the broadcast")?;
	let mut broadcast = publication.broadcast().clone();
	// Each track reserves its ceiling on the first relay's estimate, which
	// that connection then probes to fund: the bandwidth the stream is granted.
	// An allocator spans one connection, so the failover's link is never
	// probed. The catalog claims nothing itself, which would count each track
	// twice. Without a relay nothing divides any bandwidth.
	let bandwidth = match live {
		Some(live) => live.connections[0].allocator(),
		None => moq_net::bandwidth::Allocator::unlimited(),
	};
	let catalog = moq_mux::catalog::Config::default().with_catalog(Default::default());
	let catalog = moq_scene::publish::Catalog::new(&mut broadcast, catalog)?;
	let clock = catalog.clock();

	// Probed now, so a machine that cannot encode the canvas fails at startup
	// rather than once the program clock starts. Without a relay the studio
	// still composes, previews, and records audio, and a new canvas tries again.
	let probe = match Probe::new(gpu, canvas, true).await {
		Ok(probe) => Some(probe),
		Err(err) if live.is_none() => {
			stats.video.fail(format!("{err:#}"));
			None
		}
		Err(err) => return Err(err),
	};
	// Sources take the program's tier: hardware when the program found it, so a
	// session the GPU refuses fails that source loudly instead of falling to
	// software. When the program could not use hardware, such as NVENC beside an
	// integrated GPU it cannot read, a source may still reach it from the CPU.
	let kind = match probe.as_ref().map(|probe| &probe.rungs[0].config.kind) {
		Some(moq_video::encode::Kind::Named(name)) if name == "openh264" => moq_video::encode::Kind::Auto,
		_ => moq_video::encode::Kind::Hardware,
	};
	let sources = moq_scene::publish::Publish::new(clock.clone(), kind, stats.sessions.clone())?;
	// Held until the video and audio are listed, so the first catalog carries
	// both and any solicitation.
	let reserved = catalog.reserve();
	// Viewers report back beneath the broadcast, which only a relay carries.
	let viewers = match live {
		Some(_) => {
			let (echo, viewers) = health::echo(path)?;
			let mut soliciting = catalog.clone();
			let mut section = soliciting.modify()?;
			section.echo = Some(hang::catalog::Echo::new(echo));
			section.commit()?;
			Some(viewers)
		}
		None => None,
	};
	// What a recording reads the program from: its own canvas-size encoder,
	// at full quality whatever the grant, and the mix it holds, in a
	// broadcast no relay sees: announced on an origin of its own, which no
	// relay connection carries, so a recording reads it as it would the
	// published program.
	let local = moq_tokio::origin::spawn();
	let recording = local
		.create_broadcast("recording")
		.context("cannot create the recording broadcast")?;
	recording
		.announce(Default::default())
		.context("cannot announce the recording broadcast")?;
	let mut recorder = recording.broadcast().clone();
	let config = moq_mux::catalog::Config::default()
		.with_catalog(Default::default())
		.with_clock(clock.clone());
	let recorded = moq_scene::publish::Catalog::new(&mut recorder, config)?;
	let renditions = Renditions {
		gpu: gpu.clone(),
		broadcast: broadcast.clone(),
		timebase: catalog.timebase(),
		recording: Some(Recording {
			broadcast: recorder.clone(),
			timebase: recorded.timebase(),
		}),
		bandwidth: bandwidth.clone(),
		// Live on a relay and not on demand; otherwise only a viewer asks for
		// the video.
		on_demand: live.is_none_or(|live| live.on_demand),
		live: live.is_some(),
		stats: stats.clone(),
		video: stats.video.clone(),
		grants: String::new(),
	};
	let ladder = match probe {
		Some(probe) => {
			let (ladder, opened) = renditions.start(probe)?;
			// Before the program clock starts the sources, so the program holds
			// its encode session even while unwatched.
			match opened.await.context("the encode thread stopped")? {
				Ok(()) => Some(ladder),
				// Its encode thread reported why.
				Err(_) if live.is_none() => None,
				Err(err) => return Err(err),
			}
		}
		None => None,
	};

	let bus = moq_audio::bus::Config::default();
	let mut input = moq_audio::encode::Input::new(bus.sample_rate, bus.layout);
	input.format = moq_audio::Format::F32;
	let mut options = moq_audio::encode::Options::default();
	options.track = Some("audio".into());
	options.config = moq_audio::encode::Config::from_input(moq_audio::encode::Codec::Opus, &input);
	options.config.bitrate = Some(AUDIO_BITRATE);
	let graded = catalog.clone();
	let listed = catalog.clone();
	let recorded_audio = moq_audio::encode::Producer::new(&mut recorder, recorded.clone(), input.clone(), &options)?;
	let audio = moq_audio::encode::Producer::new(&mut broadcast, catalog, input, &options)?;
	let audio_reservation = bandwidth.reserve(&audio.demand(), AUDIO_BITRATE);
	stats
		.grants
		.lock()
		.unwrap()
		.insert(audio.track_name().to_string(), audio_reservation.consume());
	drop(reserved);

	// One instance on every relay, so a viewer moving between them resumes
	// mid-group rather than restarting.
	let instance = moq_net::Instance::random();
	publication
		.announce(moq_net::origin::Route::default().with_instance(instance.clone(), 0))
		.context("cannot announce the broadcast")?;

	let health = live.zip(viewers).map(|(live, viewers)| {
		let feedback = health::Feedback::default();
		let watching = health::Watching::default();
		tokio::spawn(log(
			"viewer feedback",
			feedback.clone().follow(live.reports.clone(), viewers),
		));
		tokio::spawn(log(
			"relay stats",
			watching.clone().follow(live.reports.clone(), path.clone()),
		));
		health::Publisher {
			observer: hang::health::Observer::new(path.clone(), Some(instance.clone())),
			catalog: graded,
			clock: clock.clone(),
			relays: live.links.clone(),
			feedback,
			watching,
			sources: sources.clone(),
		}
	});

	Ok((
		Program {
			_origin: origin.clone(),
			publication,
			catalog: listed,
			clock: clock.clone(),
			sources: sources.clone(),
			recording,
			_recorded: recorded,
			local,
			_audio: audio_reservation,
		},
		Publish {
			extras: Extras {
				origin: origin.clone(),
				path: path.clone(),
				clock: clock.clone(),
				instance,
				gpu: gpu.clone(),
				bandwidth,
				on_demand: renditions.on_demand,
				live: renditions.live,
				stats: stats.clone(),
			},
			renditions,
			ladder,
			audio,
			recorded_audio,
			clock,
			health,
			sources,
		},
	))
}

/// What one of the program's video encoders is for, and its place in
/// [`Stats::renditions`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
	/// The canvas size, published.
	Canvas = 0,
	/// Half of it, published.
	Half = 1,
	/// The canvas size again for a recording, in the studio's own recording
	/// broadcast, at full quality whatever the bandwidth grant.
	Record = 2,
}

impl Role {
	/// Whether viewers on a relay see it.
	fn published(self) -> bool {
		self != Self::Record
	}
}

/// Where a recording reads the program from: a broadcast the studio keeps to
/// itself, holding the recording's own canvas-size video and its own encoding
/// of the mix it holds, listed in a catalog of its own on the program's clock.
#[derive(Clone)]
struct Recording {
	broadcast: moq_net::broadcast::Producer,
	timebase: moq_mux::catalog::Timebase<moq_scene::publish::Ext>,
}

/// Where a canvas's program video is published, a canvas at a time, each
/// rendition its own track, since a track never changes what it carries.
#[derive(Clone)]
struct Renditions {
	gpu: Gpu,
	broadcast: moq_net::broadcast::Producer,
	/// Shared by every rendition, so each lands on the catalog's clock the same
	/// way and a switch keeps the video in step with the audio.
	timebase: moq_mux::catalog::Timebase<moq_scene::publish::Ext>,
	/// Where the recording's own encoder writes: the main canvas's alone.
	recording: Option<Recording>,
	/// The connection's bandwidth, which each published rendition follows its
	/// share of.
	bandwidth: bandwidth::Allocator,
	/// Whether the canvas size encodes only while someone watches.
	on_demand: bool,
	/// Whether the program goes out on a relay, where viewers can watch it.
	live: bool,
	stats: Arc<Stats>,
	/// How this canvas's video is doing.
	video: Arc<VideoStats>,
	/// What each track's grant is listed under in [`Stats::grants`], before
	/// its name: empty for the main canvas, and a further one's name.
	grants: String,
}

impl Renditions {
	/// List each of `probe`'s renditions in its catalog under a new track, and
	/// start their encode threads. The receiver reports once the canvas size's
	/// encoder opens; the half size opens its own once watched, and the
	/// recording's once recorded.
	fn start(&self, probe: Probe) -> anyhow::Result<(Ladder, tokio::sync::oneshot::Receiver<anyhow::Result<()>>)> {
		// Before the encode threads start, which report their own failures.
		*self.video.error.lock().unwrap() = None;
		for (role, load) in [Role::Half, Role::Record]
			.into_iter()
			.map(|role| (role, &self.video.renditions[role as usize]))
		{
			let refused = probe.refused.iter().find(|(refused, _)| *refused == role);
			*load.error.lock().unwrap() = refused.map(|(_, reason)| reason.clone());
			if let Some((_, reason)) = refused {
				tracing::warn!(?role, %reason, "the encoder refused a rendition");
			}
		}
		// The published sizes, largest first, each funded over the next.
		let ceilings: Vec<_> = probe
			.rungs
			.iter()
			.filter(|probed| probed.role.published())
			.map(|probed| ladder::ceiling(probed.config.size(), probed.config.framerate))
			.collect();
		let (opened, open) = tokio::sync::oneshot::channel();
		let mut opened = Some(opened);
		let number = self.video.rendition();
		let mut rungs = Vec::with_capacity(probe.rungs.len());
		for probed in probe.rungs {
			let Probed {
				role,
				exporter,
				config,
				video,
			} = probed;
			let (broadcast, timebase) = match (role, &self.recording) {
				(Role::Record, Some(recording)) => (&recording.broadcast, &recording.timebase),
				(Role::Record, None) => anyhow::bail!("only the main canvas is recorded"),
				_ => (&self.broadcast, &self.timebase),
			};
			let reserved = timebase.reserve().with_interval(config.gop.duration(config.framerate));
			let suffix = match &video.codec {
				hang::catalog::VideoCodec::H264(_) => ".avc3",
				hang::catalog::VideoCodec::H265(_) => ".hev1",
				other => anyhow::bail!("{other} is not a codec the studio publishes"),
			};
			let rank = role as usize;
			let priority = match role {
				Role::Record => hang::catalog::PRIORITY.video,
				_ => ladder::priority(rank, ceilings.len()),
			};
			let info = reserved.track_info(priority).with_refresh(ladder::REFRESH);
			let track = broadcast.unique_track(suffix, info)?;
			let producer = moq_video::encode::Producer::with_track(track.clone(), reserved, video)?;
			let (gate, grant) = match role {
				// Reads nothing from the connection, so it always encodes at its
				// ceiling, and only while recorded.
				Role::Record => {
					let gate = Gate::new(
						&bandwidth::Allocator::unlimited(),
						producer.demand(),
						ladder::ceiling(config.size(), config.framerate),
						None,
						false,
					);
					(gate, None)
				}
				_ => {
					let gate = Gate::new(
						&self.bandwidth,
						producer.demand(),
						ceilings[rank],
						ceilings.get(rank + 1).copied(),
						role == Role::Canvas && !self.on_demand,
					);
					let grant = format!("{}{}", self.grants, track.name());
					self.stats.grants.lock().unwrap().insert(grant.clone(), gate.grant());
					(gate, Some(grant))
				}
			};

			// One more than the exporter's pool, for a skip behind a full pool.
			let (frames, pending) = mpsc::sync_channel(Exporter::DEPTH + 1);
			let job = Job {
				sessions: self.stats.sessions.clone(),
				video: self.video.clone(),
				role,
				rendition: (role == Role::Canvas).then_some(number),
				zero_copy: exporter.route() == Route::Gpu,
				producer,
				track,
				config,
				// The canvas size holds its session while it skips frames.
				hold: role == Role::Canvas,
			};
			// Live, the canvas size holds its session from the start, so a
			// source past the GPU's session cap is the one that fails. Without
			// a relay nobody can watch it, so it opens nothing.
			let opened = match role {
				Role::Canvas if self.live => opened.take(),
				_ => None,
			};
			let thread = std::thread::Builder::new()
				.name("encode".into())
				.spawn(move || encode(job, opened, pending))?;
			let scaler = (role == Role::Half).then(|| Scaler::new(&self.gpu.device, &self.gpu.queue, exporter.size()));
			rungs.push(Rung {
				role,
				gate,
				grant,
				scaler,
				exporter,
				frames,
				skipped: None,
				thread,
			});
		}
		if let Some(opened) = opened {
			let _ = opened.send(Ok(()));
		}
		Ok((Ladder { rungs }, open))
	}
}

/// Encoders proven to take this GPU's export of a canvas for each of its
/// renditions, before they are listed.
struct Probe {
	/// The canvas size first.
	rungs: Vec<Probed>,
	/// Why the encoder refused a rendition the program does without.
	refused: Vec<(Role, String)>,
}

/// One rendition of a [`Probe`].
struct Probed {
	role: Role,
	exporter: Exporter,
	config: moq_video::encode::Config,
	video: hang::catalog::VideoConfig,
}

impl Probe {
	/// Open a throwaway encoder of each of `canvas`'s renditions on what `gpu`
	/// exports, the recording's copy only if `record`, which also settles
	/// whether the encoder reads the program on the GPU. The canvas size picks
	/// the encoder, which the others use too.
	async fn new(gpu: &Gpu, canvas: Canvas, record: bool) -> anyhow::Result<Self> {
		let framerate = canvas.framerate();
		let size = moq_video::Size::new(canvas.width, canvas.height);
		let sizes = ladder::sizes(size);
		let roles = [Role::Canvas, Role::Half]
			.into_iter()
			.zip(sizes)
			.chain(record.then_some((Role::Record, size)));
		let mut kind = moq_video::encode::Kind::Auto;
		let mut rungs = Vec::new();
		let mut refused = Vec::new();
		for (role, size) in roles {
			let mut config = moq_video::encode::Config::new(size.width, size.height, framerate);
			config.bitrate = Some(ladder::ceiling(size, framerate));
			config.kind = kind.clone();
			let probed = Exporter::new(&gpu.device, &gpu.queue, &mut config)
				.await
				.with_context(|| format!("cannot encode a {}x{} canvas", size.width, size.height));
			let (exporter, video) = match probed {
				Ok(probed) => probed,
				Err(err) if role == Role::Canvas => return Err(err),
				// The program goes on without it.
				Err(err) => {
					refused.push((role, format!("{err:#}")));
					continue;
				}
			};
			kind = config.kind.clone();
			rungs.push(Probed {
				role,
				exporter,
				config,
				video,
			});
		}
		Ok(Self { rungs, refused })
	}

	/// [`new`](Self::new) on a thread of its own, off the program clock, once
	/// the encode threads `ending` have closed their sessions.
	fn spawn(
		gpu: Gpu,
		canvas: Canvas,
		record: bool,
		ending: Vec<std::thread::JoinHandle<()>>,
	) -> std::io::Result<std::thread::JoinHandle<anyhow::Result<Self>>> {
		std::thread::Builder::new().name("probe".into()).spawn(move || {
			for thread in ending {
				// One that panicked has closed its session all the same.
				let _ = thread.join();
			}
			let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
			runtime.block_on(Self::new(&gpu, canvas, record))
		})
	}
}

/// One canvas's program video as the program clock feeds it: the renditions
/// of the canvas it last drew, the probe opening the next, and the encoders
/// ending.
struct Line {
	renditions: Renditions,
	ladder: Option<Ladder>,
	/// The canvas `ladder` is for, or will be once probed.
	canvas: Canvas,
	/// One probe at a time, with the canvas it is for, since a GPU caps its
	/// encode sessions.
	probing: Option<(Canvas, std::thread::JoinHandle<anyhow::Result<Probe>>)>,
	/// The encode threads of renditions ended or failed since, whose sessions
	/// the next probe waits to be closed.
	ending: Vec<std::thread::JoinHandle<()>>,
	/// Whether the canvas drawn has its renditions, or failed to open them, so
	/// a failure is not retried until the canvas changes.
	settled: bool,
}

/// What a [`Line`] leaves running once ended, to wait on.
struct Ended {
	threads: Vec<std::thread::JoinHandle<()>>,
	/// Holds the encode threads it was waiting on.
	probe: Option<std::thread::JoinHandle<anyhow::Result<Probe>>>,
	/// A further canvas's broadcast, dropped only after the threads, so its
	/// catalog finishes after the video it lists.
	_broadcast: Option<Broadcast>,
}

impl Ended {
	fn is_finished(&self) -> bool {
		self.threads.iter().all(std::thread::JoinHandle::is_finished)
			&& self.probe.as_ref().is_none_or(std::thread::JoinHandle::is_finished)
	}

	fn join(self) {
		for thread in self.threads {
			if thread.join().is_err() {
				tracing::error!("an encode thread panicked");
			}
		}
		if let Some(probe) = self.probe {
			let _ = probe.join();
		}
	}
}

impl Line {
	/// `renditions` of `canvas`: `ladder` when already open, or `None` to probe
	/// for them on the first feed unless `settled`, as a canvas that failed
	/// to open at startup is.
	fn new(renditions: Renditions, canvas: Canvas, ladder: Option<Ladder>, settled: bool) -> Self {
		if let Some(ladder) = &ladder {
			ladder.adopted(&renditions.video);
		}
		Self {
			renditions,
			ladder,
			canvas,
			probing: None,
			ending: Vec::new(),
			settled,
		}
	}

	/// Hand `program`, drawn at `drawn`, to each rendition its gate lets
	/// encode, stamped `timestamp`. A new canvas size or frame rate ends the
	/// renditions and probes for others, since a track never changes what it
	/// carries.
	fn feed(&mut self, program: &wgpu::Texture, drawn: Canvas, timestamp: Option<moq_net::Timestamp>) {
		let renditions = &self.renditions;
		let video = &renditions.video;
		if drawn != self.canvas {
			self.canvas = drawn;
			self.settled = false;
			// Ended before the next open, so the two never hold encode sessions
			// at once.
			self.ending
				.extend(self.ladder.take().into_iter().flat_map(|ladder| ladder.end(renditions)));
			video.retire_encoder();
		}
		// Probed beside the program clock, so drawing carries on.
		if let Some((probed, probe)) = self.probing.take_if(|(_, probe)| probe.is_finished()) {
			let probe = probe
				.join()
				.unwrap_or_else(|_| Err(anyhow::anyhow!("the encoder probe panicked")));
			// One for a canvas since left is dropped unlisted.
			if probed == self.canvas {
				self.settled = true;
				match probe.and_then(|probe| renditions.start(probe)) {
					Ok((started, _)) => {
						started.adopted(video);
						self.ladder = Some(started);
					}
					Err(err) => video.fail(format!("{err:#}")),
				}
			}
		}
		if self.probing.is_none() && !self.settled {
			let ending = std::mem::take(&mut self.ending);
			let record = renditions.recording.is_some();
			match Probe::spawn(renditions.gpu.clone(), self.canvas, record, ending) {
				Ok(probe) => self.probing = Some((self.canvas, probe)),
				Err(err) => {
					self.settled = true;
					video.fail(format!("cannot start an encoder probe: {err}"));
				}
			}
		}

		let Some(current) = &mut self.ladder else {
			video.watched.store(false, Ordering::Relaxed);
			video.encoding.store(false, Ordering::Relaxed);
			return;
		};
		let watched = current
			.rungs
			.iter()
			.any(|rung| rung.role.published() && rung.gate.watched());
		video.watched.store(watched, Ordering::Relaxed);
		let Some(timestamp) = timestamp else { return };
		let mut encoding = false;
		let mut failed = Vec::new();
		for (index, rung) in current.rungs.iter_mut().enumerate() {
			let role = rung.role;
			match rung.feed(&video.renditions[role as usize], program, timestamp) {
				Some(step) => encoding |= role == Role::Canvas && matches!(step, Step::Encode(_)),
				None => failed.push((index, role)),
			}
		}
		video.encoding.store(encoding, Ordering::Relaxed);
		// An encoder stops only having reported why. Without the canvas size
		// the video stops; without another rendition it carries on.
		if failed.iter().any(|(_, role)| *role == Role::Canvas) {
			self.ending
				.extend(self.ladder.take().into_iter().flat_map(|ladder| ladder.end(renditions)));
		} else {
			for (index, _) in failed.into_iter().rev() {
				self.ending.push(current.remove(index, renditions));
			}
		}
	}

	/// Stop handing the renditions frames, so each encode thread publishes
	/// what it holds and finishes its track, then ends a further canvas's
	/// `broadcast`.
	fn end(mut self, broadcast: Option<Broadcast>) -> Ended {
		let mut threads = std::mem::take(&mut self.ending);
		threads.extend(
			self.ladder
				.take()
				.into_iter()
				.flat_map(|ladder| ladder.end(&self.renditions)),
		);
		self.renditions.video.watched.store(false, Ordering::Relaxed);
		self.renditions.video.encoding.store(false, Ordering::Relaxed);
		Ended {
			threads,
			probe: self.probing.map(|(_, probe)| probe),
			_broadcast: broadcast,
		}
	}
}

/// Each further canvas's line and broadcast, by name, or `None` once it failed
/// to start, which is not retried until the canvas is removed and added again.
type Lines = BTreeMap<String, Option<(Line, Broadcast)>>;

/// A further canvas's broadcast, whose catalog finishes and which closes
/// when dropped.
struct Broadcast {
	publication: moq_net::origin::Publication,
	/// Lists the canvas's video and the main broadcast's audio.
	catalog: moq_scene::publish::Catalog,
}

impl Drop for Broadcast {
	fn drop(&mut self) {
		// Fails only once the catalog failed, which ended its track already.
		let _ = self.catalog.finish();
		self.publication.close();
	}
}

/// What starts each further canvas's broadcast: beneath the main one's path,
/// on its clock, sharing its connection's bandwidth.
struct Extras {
	origin: moq_net::origin::Producer,
	path: moq_net::Path,
	clock: moq_mux::Clock,
	/// The main broadcast's, which its audio is listed by.
	instance: moq_net::Instance,
	gpu: Gpu,
	bandwidth: bandwidth::Allocator,
	on_demand: bool,
	live: bool,
	stats: Arc<Stats>,
}

impl Extras {
	/// Publish the further canvas `name`, first drawn at `canvas`, and the
	/// [`Line`] that encodes its video, reporting in `video`. Its encoders are
	/// probed for on the line's first feed, off the program clock, for a canvas
	/// added while the studio runs.
	fn start(
		&self,
		name: &str,
		canvas: Canvas,
		video: Arc<VideoStats>,
		program: &Program,
	) -> anyhow::Result<(Line, Broadcast)> {
		let (renditions, broadcast) = self.publish(name, video, program)?;
		Ok((Line::new(renditions, canvas, None, false), broadcast))
	}

	/// [`start`](Self::start), opening the canvas size's encoder before
	/// returning, as the main canvas does at startup, so a canvas in the scene
	/// file holds its encode session before any source opens one. A canvas
	/// whose encoder cannot open draws on without video, saying why in `video`.
	async fn open(
		&self,
		name: &str,
		canvas: Canvas,
		video: Arc<VideoStats>,
		program: &Program,
	) -> anyhow::Result<(Line, Broadcast)> {
		let (renditions, broadcast) = self.publish(name, video.clone(), program)?;
		let opened = async {
			let probe = Probe::new(&self.gpu, canvas, false).await?;
			let (ladder, opened) = renditions.start(probe)?;
			opened.await.context("the encode thread stopped")??;
			anyhow::Ok(ladder)
		};
		let ladder = match opened.await {
			Ok(ladder) => Some(ladder),
			Err(err) => {
				video.fail(format!("{err:#}"));
				None
			}
		};
		Ok((Line::new(renditions, canvas, ladder, true), broadcast))
	}

	/// Publish the further canvas `name` at its own path beneath the main
	/// broadcast's, listing the main broadcast's audio by reference so it is
	/// encoded once, and where its video goes, reporting in `video`.
	fn publish(
		&self,
		name: &str,
		video: Arc<VideoStats>,
		program: &Program,
	) -> anyhow::Result<(Renditions, Broadcast)> {
		let path = moq_net::Path::new(name)
			.and_then(|name| self.path.join(&name))
			.with_context(|| format!("cannot publish canvas `{name}`"))?;
		if self.live {
			let (_, echo) = health::echo(&self.path)?;
			anyhow::ensure!(
				path != echo,
				"canvas `{name}` would publish where viewers report back; remove it and add it under another name"
			);
		}
		let audio = self
			.path
			.relative(&path)
			.with_context(|| format!("canvas `{name}` cannot refer to the main broadcast"))?;

		let publication = self
			.origin
			.create_broadcast(&path)
			.with_context(|| format!("cannot create the broadcast of canvas `{name}`"))?;
		let mut broadcast = publication.broadcast().clone();
		let config = moq_mux::catalog::Config::default()
			.with_catalog(Default::default())
			.with_clock(self.clock.clone());
		let mut catalog = moq_scene::publish::Catalog::new(&mut broadcast, config)?;
		let listed = program.catalog.snapshot().audio.renditions;
		catalog.mutate(|catalog| {
			for (id, mut config) in listed {
				config.broadcast = Some(audio.clone());
				config.instance = Some(self.instance.clone());
				config.track.get_or_insert_with(|| id.clone());
				catalog.audio.renditions.insert(id, config);
			}
		})?;
		publication
			.announce(moq_net::origin::Route::default().with_instance(moq_net::Instance::random(), 0))
			.with_context(|| format!("cannot announce the broadcast of canvas `{name}`"))?;

		let renditions = Renditions {
			gpu: self.gpu.clone(),
			broadcast,
			timebase: catalog.timebase(),
			recording: None,
			bandwidth: self.bandwidth.clone(),
			on_demand: self.on_demand,
			live: self.live,
			stats: self.stats.clone(),
			video,
			grants: format!("{name}/"),
		};
		let broadcast = Broadcast { publication, catalog };
		Ok((renditions, broadcast))
	}
}

/// The program video of one canvas: each of its renditions, the canvas size
/// first.
struct Ladder {
	rungs: Vec<Rung>,
}

impl Ladder {
	/// Report these renditions as the canvas's from now on.
	fn adopted(&self, video: &VideoStats) {
		let zero_copy = self.rungs[0].exporter.route() == Route::Gpu;
		video.zero_copy.store(zero_copy, Ordering::Relaxed);
		self.count(video);
	}

	/// Report how many sizes the canvas publishes.
	fn count(&self, video: &VideoStats) {
		let sizes = self.rungs.iter().filter(|rung| rung.role.published()).count();
		video.sizes.store(sizes, Ordering::Relaxed);
	}

	/// Stop handing the renditions frames, so each encode thread encodes those
	/// it holds, finishes its track, and returns.
	fn end(self, renditions: &Renditions) -> Vec<std::thread::JoinHandle<()>> {
		renditions.video.sizes.store(0, Ordering::Relaxed);
		let mut grants = renditions.stats.grants.lock().unwrap();
		self.rungs
			.into_iter()
			.map(|rung| {
				if let Some(grant) = &rung.grant {
					grants.remove(grant);
				}
				rung.thread
			})
			.collect()
	}

	/// Stop handing rendition `index` frames, as [`end`](Self::end) does.
	/// The canvas size left alone no longer disables below the half size,
	/// since no viewer has a smaller one to move to.
	fn remove(&mut self, index: usize, renditions: &Renditions) -> std::thread::JoinHandle<()> {
		let rung = self.rungs.remove(index);
		if let Some(grant) = &rung.grant {
			renditions.stats.grants.lock().unwrap().remove(grant);
		}
		self.count(&renditions.video);
		if rung.role == Role::Half {
			for canvas in self.rungs.iter_mut().filter(|rung| rung.role == Role::Canvas) {
				canvas.gate.alone();
			}
		}
		rung.thread
	}
}

/// One rendition of the program video, as the program clock feeds it: its
/// gate, the exporter it copies into, and the encode thread behind it.
struct Rung {
	role: Role,
	gate: Gate,
	/// Its grant's name in [`Stats::grants`]: a published rendition's alone.
	grant: Option<String>,
	/// Draws the program at the rendition's size; `None` at the canvas size.
	scaler: Option<Scaler>,
	exporter: Exporter,
	frames: mpsc::SyncSender<Video>,
	/// What the encoder was last told while skipping, so it is told once.
	skipped: Option<Step>,
	thread: std::thread::JoinHandle<()>,
}

impl Rung {
	/// Hand the encoder `program` at `timestamp`, or tell it why not, counting
	/// in `load`. Returns what the gate decided, or `None` once the encoder
	/// stopped, having reported why.
	fn feed(&mut self, load: &Load, program: &wgpu::Texture, timestamp: moq_net::Timestamp) -> Option<Step> {
		let step = self.gate.step();
		let message = match step {
			Step::Encode(bitrate) => {
				self.skipped = None;
				let texture = match &self.scaler {
					Some(scaler) => scaler.scale(program),
					None => program,
				};
				match self.exporter.copy(texture, timestamp) {
					// The pool bounds the frames in flight, so this waits only
					// behind a skip the encoder has not reached yet.
					Ok(Some(pending)) => Some(Video::Frame(pending, bitrate)),
					Ok(None) => {
						load.dropped.fetch_add(1, Ordering::Relaxed);
						None
					}
					// The canvas and the exporter share a size and format, so this
					// is the GPU failing: the video stops, and the canvas keeps
					// drawing.
					Err(err) => {
						let _ = self.frames.send(Video::Failed(err));
						return None;
					}
				}
			}
			skip => (self.skipped.replace(skip) != Some(skip)).then_some(Video::Skip(skip)),
		};
		match message {
			Some(message) => self.frames.send(message).ok().map(|()| step),
			None => Some(step),
		}
	}
}

/// How often the program is drawn at `framerate`, from `start`.
fn program_clock(start: Instant, framerate: u32) -> kio::clock::Interval {
	let mut clock = kio::clock::interval_at(start, period(framerate));
	clock.set_missed_tick_behavior(kio::clock::MissedTickBehavior::Skip);
	clock
}

fn period(framerate: u32) -> Duration {
	Duration::from_secs(1) / framerate
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
			// The first relay's route to a broadcast source wins while it has
			// one, whether it carries the same instance as the failover's or
			// another.
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

/// What the program clock hands a rendition's encoder.
enum Video {
	/// A copy of the program on its way to the encoder, to encode at this
	/// bitrate.
	Frame(Pending, bandwidth::Rate),
	/// Frames stop until the gate lets the rendition encode again: nobody
	/// watches, or the grant cannot fund it.
	Skip(Step),
	/// The program could not be copied, so the video stops.
	Failed(moq_video::Error),
}

/// What an encode thread runs.
struct Job {
	sessions: Sessions,
	/// How its canvas's video is doing.
	video: Arc<VideoStats>,
	/// Which of the program's renditions it is.
	role: Role,
	/// The number of the program's renditions to report the encoder under:
	/// `Some` for the canvas size alone.
	rendition: Option<u64>,
	/// Whether frames reach the encoder as GPU surfaces.
	zero_copy: bool,
	producer: moq_video::encode::Producer,
	/// The producer's track, to see who joins it.
	track: moq_net::track::Producer,
	config: moq_video::encode::Config,
	/// Keep the encoder, and its session, while skipping frames.
	hold: bool,
}

/// One rendition's encoder and track, doing what its gate decided.
struct Encoder {
	track: Encoding,
	config: moq_video::encode::Config,
	joins: Joins,
	/// Where to report each encoder opened as its canvas's, under the
	/// number of the canvas's renditions: the canvas size's alone.
	report: Option<(Arc<VideoStats>, u64)>,
	/// Keep the encoder, and its session, while skipping frames.
	hold: bool,
	/// Whether the open encoder can cut a keyframe for a joining viewer.
	cuts: bool,
	/// The open encoder cannot retune, so it stays at its ceiling.
	fixed: bool,
	/// Frames were encoded since the last skip.
	encoding: bool,
	/// The catalog advertises the rendition as encoding.
	enabled: bool,
}

impl Encoder {
	/// Open an encoder, returning its backend's name.
	fn open(&mut self) -> anyhow::Result<()> {
		let mut encoder = moq_video::encode::Encoder::new(&self.config)?;
		// Asked of a fresh encoder, whose first picture is a keyframe anyway, so
		// the cut it queues costs nothing.
		self.cuts = match encoder.cut() {
			Ok(()) => true,
			Err(moq_video::Error::CutUnsupported(name)) => {
				tracing::warn!(
					encoder = name,
					"the encoder cannot cut, so a joining viewer waits for its next keyframe"
				);
				false
			}
			Err(err) => return Err(err.into()),
		};
		self.fixed = false;
		if let Some((stats, rendition)) = &self.report {
			stats.opened_encoder(*rendition, encoder.name());
		}
		self.track.open(encoder)?;
		Ok(())
	}

	/// Encode `frame` at `bitrate`, opening an encoder if none is.
	fn encode(&mut self, frame: &moq_video::Frame, bitrate: bandwidth::Rate) -> anyhow::Result<()> {
		if !self.track.is_open() {
			self.open().context("video encoder")?;
		}
		if !self.enabled {
			// With the keyframe that resumes it.
			self.track.set_enabled(true)?;
			self.enabled = true;
		}
		self.encoding = true;
		self.retune(bitrate)?;
		if self.cuts && self.joins.due() {
			self.track.cut()?;
		}
		self.track.encode(frame)?;
		Ok(())
	}

	/// Skip frames as `step` says, marking the gap once, so a viewer arriving
	/// during it is not served the last group before it as if it were live,
	/// and one arriving after it starts from a keyframe.
	fn skip(&mut self, step: Step) -> anyhow::Result<()> {
		if std::mem::take(&mut self.encoding) {
			match self.hold && self.cuts {
				true => self.track.pause()?,
				// Gives the session back.
				false => self.track.discontinuity()?,
			}
		}
		let enabled = step != Step::Disabled;
		if self.enabled != enabled {
			self.track.set_enabled(enabled)?;
			self.enabled = enabled;
		}
		Ok(())
	}

	/// Move the encoder to `bitrate`, unless it cannot retune.
	fn retune(&mut self, bitrate: bandwidth::Rate) -> anyhow::Result<()> {
		if self.fixed {
			return Ok(());
		}
		match self.track.set_bitrate(bitrate) {
			Ok(()) => Ok(()),
			// Not fatal: it keeps encoding at its ceiling, and only stops
			// following the grant.
			Err(moq_scene::Error::Video(moq_video::Error::BitrateUnsupported(name))) => {
				tracing::warn!(encoder = name, "the encoder cannot follow the bandwidth grant");
				self.fixed = true;
				Ok(())
			}
			Err(err) => Err(err.into()),
		}
	}
}

/// Run a rendition's encoder: with `opened`, open it first and report how
/// that went there, then encode program frames until the program clock ends
/// the rendition, and finish its track.
///
/// A failure aborts the track, so viewers see that it failed rather than a
/// track that went away unfinished.
fn encode(job: Job, opened: Option<tokio::sync::oneshot::Sender<anyhow::Result<()>>>, video: mpsc::Receiver<Video>) {
	let Job {
		sessions,
		video: stats,
		role,
		rendition,
		zero_copy,
		producer,
		track,
		config,
		hold,
	} = job;
	let size = config.size();
	// Built here, since an encoder stays on the thread that opened it.
	let mut encoder = Encoder {
		track: Encoding::new(producer, sessions),
		joins: Joins::new(track),
		report: rendition.map(|rendition| (stats.clone(), rendition)),
		config,
		hold,
		cuts: false,
		fixed: false,
		encoding: false,
		enabled: true,
	};
	// The canvas size failing stops the video; another rendition failing
	// leaves it to the canvas size.
	let load = &stats.renditions[role as usize];
	let fail = |err: &anyhow::Error| match role {
		Role::Canvas => stats.fail(format!("{err:#}")),
		_ => load.fail(format!("{}x{}: {err:#}", size.width, size.height)),
	};
	if let Some(opened) = opened {
		match encoder.open() {
			Ok(()) => {
				let _ = opened.send(Ok(()));
			}
			Err(err) => {
				let err = err.context("video encoder");
				fail(&err);
				encoder.track.abort(moq_net::StreamError::Internal.into());
				let _ = opened.send(Err(err));
				return;
			}
		}
	}

	match encode_frames(load, video, &mut encoder, zero_copy) {
		// Publishes what the encoder still holds before the track ends, so a
		// rendition's last pictures are not lost to a resize or to quitting. A
		// failure aborts it.
		Ok(()) => {
			if let Err(err) = encoder.track.finish() {
				fail(&err.into());
			}
		}
		Err(err) => {
			fail(&err);
			encoder.track.abort(moq_net::StreamError::Internal.into());
		}
	}
}

fn encode_frames(
	load: &Load,
	video: mpsc::Receiver<Video>,
	encoder: &mut Encoder,
	zero_copy: bool,
) -> anyhow::Result<()> {
	while let Ok(video) = video.recv() {
		let (pending, bitrate) = match video {
			Video::Frame(pending, bitrate) => (pending, bitrate),
			Video::Skip(step) => {
				encoder.skip(step).context("video")?;
				continue;
			}
			Video::Failed(err) => return Err(err).context("copying the program"),
		};
		let start = Instant::now();
		let frame = pending.wait().context("copying the program")?;
		load.copy_us
			.store(start.elapsed().as_micros() as u64, Ordering::Relaxed);
		load.copied.fetch_add(1, Ordering::Relaxed);
		if !zero_copy {
			load.read_back.fetch_add(1, Ordering::Relaxed);
		}

		let start = Instant::now();
		encoder.encode(&frame, bitrate).context("video")?;
		load.encode_us
			.store(start.elapsed().as_micros() as u64, Ordering::Relaxed);
		load.encoded.fetch_add(1, Ordering::Relaxed);
	}
	Ok(())
}

/// What browser sources run in: this executable, which `main` hands to
/// `moq_scene::browser::child` when it is started as their helper, on the
/// user's browser profile.
fn browser(gpu: &Gpu) -> anyhow::Result<Option<moq_scene::browser::Helper>> {
	if !cfg!(feature = "cef") {
		return Ok(None);
	}
	let program = std::env::current_exe().context("cannot find the studio's executable")?;
	Ok(Some(moq_scene::browser::Helper::new(
		program,
		browser_profile()?,
		&gpu.adapter,
		&gpu.device,
		&gpu.queue,
	)))
}

/// Where browser sources keep their cookies and logins, whichever scene file
/// is open: `moq-studio/browser` in the user's data directory, as the XDG base
/// directories name it, since browser sources run on Linux only for now.
fn browser_profile() -> anyhow::Result<PathBuf> {
	let data = match std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
		// The spec ignores a relative one.
		Some(data) if data.is_absolute() => data,
		_ => {
			let home = std::env::var_os("HOME").context("no HOME to keep the browser profile in")?;
			PathBuf::from(home).join(".local/share")
		}
	};
	Ok(data.join("moq-studio/browser"))
}

/// Run a source on its own thread until its feed is dropped, or fail the
/// source when its thread cannot start. Returns what closes as the source
/// ends, having finished its own track.
fn spawn(driver: Driver) -> tokio::sync::oneshot::Receiver<()> {
	// Handed over once the thread exists, since a failed spawn drops what it
	// was given and the driver must stay here to report why.
	let (handoff, received) = mpsc::sync_channel::<Driver>(1);
	let (running, ended) = tokio::sync::oneshot::channel::<()>();
	let name = format!("source-{}", driver.name());
	let started = clocked(name, async move || {
		let _running = running;
		if let Ok(driver) = received.recv() {
			driver.run().await;
		}
	});
	match started {
		Ok(_) => {
			if let Err(mpsc::SendError(driver)) = handoff.send(driver) {
				driver.fail("the source thread stopped");
			}
		}
		Err(err) => driver.fail(format!("{err:#}")),
	}
	ended
}

/// Start a thread called `name` that runs `task` with its own clock and IO,
/// returning why if either cannot start.
fn clocked(name: String, task: impl AsyncFnOnce() + Send + 'static) -> anyhow::Result<std::thread::JoinHandle<()>> {
	// The thread starts before its runtime is built and handed over, since a
	// runtime dropped in the async caller, as a refused spawn would, panics.
	let (handoff, received) = mpsc::sync_channel::<tokio::runtime::Runtime>(1);
	let thread = std::thread::Builder::new()
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
	Ok(thread)
}

#[cfg(test)]
mod tests {
	use moq_mux::catalog::Stream;

	use super::*;

	/// Bounds each step, so a stall fails rather than hangs.
	const TIMEOUT: Duration = Duration::from_secs(30);

	/// How the tests start a studio, live on `relays`.
	fn launch(gpu: Gpu, relays: Vec<Relay>, recordings: &Path) -> Launch {
		Launch {
			gpu,
			relays,
			broadcast: "studio".into(),
			on_demand: false,
			recordings: recordings.to_path_buf(),
			room: None,
		}
	}

	/// A small canvas filled red, at two sizes.
	fn red_canvas() -> autosave::Loaded {
		let mut collection = Collection::new();
		collection.canvas = moq_scene::Canvas {
			width: 640,
			height: 360,
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
		autosave::Loaded {
			collection,
			history: History::new(),
			dropped: 0,
		}
	}

	/// Program frames the recording's own encoder encoded.
	fn recorded(engine: &Engine) -> u64 {
		engine.stats.video.renditions[Role::Record as usize]
			.encoded
			.load(Ordering::Relaxed)
	}

	/// Wait until the recording's own encoder encoded `frames` program frames.
	async fn encoded(engine: &Engine, frames: u64) {
		kio::clock::timeout(TIMEOUT, async {
			while recorded(engine) < frames {
				kio::clock::sleep(Duration::from_millis(100)).await;
			}
		})
		.await
		.expect("the recording encodes the program");
	}

	/// Finish the recording, and return the handler of every track holding
	/// samples in each file, by file.
	async fn finished(recorder: &record::Recorder) -> Vec<Vec<String>> {
		recorder.stop();
		done(recorder).await
	}

	/// Wait for the recording to finish, and return the handler of every
	/// track holding samples in each file, by file.
	async fn done(recorder: &record::Recorder) -> Vec<Vec<String>> {
		kio::clock::timeout(TIMEOUT, recorder.done())
			.await
			.expect("the recording finishes");

		use moq_mux::container::fmp4::mp4_atom::{self, DecodeMaybe};
		let status = recorder.status().expect("a recording");
		status
			.files
			.iter()
			.map(|file| {
				assert_eq!(file.finished, Some(record::Finished::Saved), "{}", file.path.display());
				let data = std::fs::read(&file.path).unwrap();
				assert_eq!(file.written, data.len() as u64);
				let mut cursor = std::io::Cursor::new(data.as_slice());
				assert!(
					moq_mux::container::mp4::repair(&mut cursor).unwrap().is_none(),
					"a regular MP4"
				);
				let mut cursor = std::io::Cursor::new(data.as_slice());
				let mut handlers = Vec::new();
				while let Some(atom) = mp4_atom::Any::decode_maybe(&mut cursor).unwrap() {
					if let mp4_atom::Any::Moov(moov) = atom {
						let sampled = moov.trak.iter().filter(|trak| match &trak.mdia.minf.stbl.stsz.samples {
							mp4_atom::StszSamples::Identical { count, .. } => *count > 0,
							mp4_atom::StszSamples::Different { sizes } => !sizes.is_empty(),
						});
						handlers.extend(sampled.map(|trak| trak.mdia.hdlr.handler.to_string()));
					}
				}
				handlers.sort();
				handlers
			})
			.collect()
	}

	/// Without a relay, a recording encodes the program on its own and saves it
	/// as a regular MP4 holding its video and audio. The color source publishes
	/// a still, which has no file of its own. A new canvas goes on in another
	/// file, holding the new rendition's video. A recording holding a mix
	/// other than the one on program records its audio.
	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "needs a Vulkan device and an H.264 encoder; run by just rs render-sw"]
	async fn the_program_records_without_a_relay() {
		let dir = tempfile::tempdir().unwrap();
		let gpu = Gpu::new().await.expect("a GPU");
		let (autosave, _) = Autosave::open(dir.path().join("show.toml")).unwrap();
		let engine = Engine::start(launch(gpu, Vec::new(), dir.path()), autosave, red_canvas())
			.await
			.expect("the studio starts");
		assert!(engine.output.is_none());
		assert_eq!(recorded(&engine), 0);
		assert_eq!(
			engine.stats.sessions.open(),
			0,
			"nothing encodes without a relay or a recording"
		);

		engine.recorder.start(true).expect("recording starts");
		// Long enough for the first group to close, so the video has a fragment.
		encoded(&engine, 90).await;
		// Only the recording's own encoder: nobody watches the published sizes.
		assert_eq!(engine.stats.video.renditions[0].encoded.load(Ordering::Relaxed), 0);
		assert_eq!(engine.stats.sessions.open(), 1);
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
		encoded(&engine, 90 + 45).await;
		let files = finished(&engine.recorder).await;
		assert_eq!(files, [["soun", "vide"], ["soun", "vide"]]);
		let status = engine.recorder.status().unwrap();
		let name = status.files[1].path.file_name().unwrap().to_string_lossy();
		assert!(name.ends_with("Z (2).mp4"), "{name}");

		{
			let mut state = engine.state.lock().unwrap();
			let mut mixes = state.collection().mixes.clone();
			mixes.names.push("Full".into());
			mixes.recording = Some("Full".into());
			let mut draft = state.collection().draft("Record `Full`");
			draft.set_mixes(mixes).unwrap();
			state.commit(draft, None).unwrap();
		}

		// Quitting while recording reads the program until stopping ends its
		// tracks, into the one file.
		engine.recorder.start(false).expect("recording starts");
		let frames = recorded(&engine);
		encoded(&engine, frames + 30).await;
		engine.recorder.close();
		let stopping = engine.clone();
		tokio::task::spawn_blocking(move || stopping.stop()).await.unwrap();
		let files = done(&engine.recorder).await;
		assert_eq!(files, [["soun", "vide"]]);
		assert!(engine.stats.error().is_none());
	}

	/// The clip a stinger plays rests ended between stingers without Health
	/// counting it, while another media file that ended and is heard counts,
	/// as does the clip once a scene on air places it. The program shows the
	/// clip, and what its source's mask reads, while a stinger plays it.
	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "needs a Vulkan device; run by just rs render-sw"]
	async fn the_program_rests_a_stinger_clip() {
		let dir = tempfile::tempdir().unwrap();
		let clip = include_bytes!("../../moq-scene/src/test_data/clip.mkv");
		let mut loaded = red_canvas();
		for name in ["sting", "bed"] {
			let path = format!("{name}.mkv");
			std::fs::write(dir.path().join(&path), clip).unwrap();
			let media = moq_scene::Source::Media {
				path: path.into(),
				looping: false,
				restart: false,
			};
			loaded.collection.sources.insert(name.into(), media);
		}
		loaded.collection.transition = moq_scene::Transition::Stinger {
			source: "sting".into(),
			cut: Duration::from_millis(300),
		};
		let matte = moq_scene::Source::Color {
			color: moq_scene::Rgba([255, 255, 255, 255]),
		};
		loaded.collection.sources.insert("matte".into(), matte);
		let mask = moq_scene::Filter::Mask {
			source: "matte".into(),
			mode: moq_scene::MaskMode::Alpha,
		};
		let video = moq_scene::Video {
			filters: vec![mask],
			..Default::default()
		};
		loaded.collection.video.insert("sting".into(), video);
		let gpu = Gpu::new().await.expect("a GPU");
		let (autosave, _) = Autosave::open(dir.path().join("show.toml")).unwrap();
		let engine = Engine::start(launch(gpu, Vec::new(), dir.path()), autosave, loaded)
			.await
			.expect("the studio starts");
		let status = |name: &str| engine.state.lock().unwrap().sources.get(name).map(source::Feed::status);
		let ended = || async {
			while status("bed") != Some(Status::Ended) || status("sting") != Some(Status::Ended) {
				kio::clock::sleep(Duration::from_millis(10)).await;
			}
		};
		kio::clock::timeout(TIMEOUT, ended()).await.expect("the bed ends");
		let stopped = engine.local(kio::clock::now()).stopped;
		assert_eq!(stopped, [("bed".to_string(), Status::Ended)]);

		{
			let mut state = engine.state.lock().unwrap();
			let transition = state.collection().transition.clone();
			let state = &mut *state;
			state
				.compositor
				.transition(transition, &state.sources, kio::clock::now());
			let shown = state.shown();
			assert!(shown.contains("sting") && shown.contains("matte"), "{shown:?}");
		}
		let played = || async {
			while engine.state.lock().unwrap().compositor.transitioning() {
				kio::clock::sleep(Duration::from_millis(10)).await;
			}
		};
		kio::clock::timeout(TIMEOUT, played()).await.expect("the stinger ends");
		let shown = engine.state.lock().unwrap().shown();
		assert!(!shown.contains("sting") && !shown.contains("matte"), "{shown:?}");
		kio::clock::timeout(TIMEOUT, ended()).await.expect("the clip rests");
		let stopped = engine.local(kio::clock::now()).stopped;
		assert_eq!(stopped, [("bed".to_string(), Status::Ended)]);

		{
			let mut state = engine.state.lock().unwrap();
			let scene = state.compositor.program_scene().unwrap().to_string();
			let fill = moq_scene::Rect::fill(&state.collection().canvas);
			let mut draft = state.collection().draft("Place sting");
			let item = moq_scene::Item::new("sting", fill);
			draft.insert_item(&moq_scene::Layer::scene(scene), 1, item).unwrap();
			state.commit(draft, None).unwrap();
		}
		let stopped = engine.local(kio::clock::now()).stopped;
		let both = [("bed".to_string(), Status::Ended), ("sting".to_string(), Status::Ended)];
		assert_eq!(stopped, both, "placed, it counts");

		let stopping = engine.clone();
		tokio::task::spawn_blocking(move || stopping.stop()).await.unwrap();
	}

	/// A rendition a resize retired while its encoder was still opening must
	/// not report that encoder over the next rendition's.
	#[test]
	fn a_retired_rendition_reports_no_encoder() {
		let stats = VideoStats::default();
		let retired = stats.rendition();
		stats.retire_encoder();
		let next = stats.rendition();
		stats.opened_encoder(retired, "old");
		assert_eq!(stats.encoder(), None, "the retired encoder opened after the resize");
		stats.opened_encoder(next, "new");
		assert_eq!(stats.encoder().as_deref(), Some("new"));
	}

	/// No edit removes a scene on air: not a remove, nor an undo of the add
	/// that made it, even after a rename, while any other scene can go.
	#[test]
	fn a_scene_on_air_stays() {
		let mut collection = Collection::new();
		let live = collection.scenes[0].name.clone();
		let mut draft = collection.draft("Add `Other`");
		draft.insert_scene(1, moq_scene::Scene::new("Other")).unwrap();
		let add = draft.finish().unwrap();
		collection.apply(&add).unwrap();
		let mut draft = collection.draft("Rename `Other`");
		draft.rename_scene(1, "Renamed").unwrap();
		let rename = draft.finish().unwrap();
		let added = collection.clone();
		collection.apply(&rename).unwrap();

		let remove = |collection: &Collection, index| {
			let mut draft = collection.draft("Remove");
			draft.remove_scene(index).unwrap();
			collection.renames(&draft.finish().unwrap())
		};
		let removed = [remove(&collection, 0)];
		assert!(keep_on_air([live.as_str()], &removed).is_err());
		assert!(keep_on_air(["Renamed", live.as_str()], &removed).is_err(), "leaving");
		keep_on_air([live.as_str()], &[remove(&collection, 1)]).unwrap();
		keep_on_air([], &removed).unwrap();

		// Undoing both edits, newest first, removes `Other` once renamed.
		let undo = [collection.renames(&rename.inverse()), added.renames(&add.inverse())];
		let err = keep_on_air(["Renamed"], &undo).unwrap_err();
		assert!(err.to_string().contains("`Other` is on air"), "{err}");
		keep_on_air([live.as_str()], &undo).unwrap();
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

	/// The broadcast of the further canvas `tall` as `relay` routes it, its
	/// catalogs, and the first that lists its video.
	async fn routed_tall(
		relay: &moq_net::origin::Consumer,
	) -> (
		moq_net::broadcast::Consumer,
		moq_mux::catalog::Consumer<()>,
		hang::Catalog,
	) {
		// Until the canvas's broadcast is announced, the main one's route covers
		// its path and cannot serve it; the follow moves on once it is.
		let mut following = relay.follow("studio/tall", None).unwrap();
		let (broadcast, mut catalogs) = kio::clock::timeout(TIMEOUT, async {
			loop {
				let broadcast = following.available().await.expect("broadcast");
				let format = moq_mux::catalog::CatalogFormat::default();
				if let Ok(catalogs) = moq_mux::catalog::Consumer::<()>::new(&broadcast, format).await {
					return (broadcast, catalogs);
				}
			}
		})
		.await
		.expect("the relay routes the canvas");
		// Audio is listed from the start, and the video once its encoder opens.
		let catalog = kio::clock::timeout(TIMEOUT, async {
			loop {
				let catalog = catalogs.next().await.expect("catalog").expect("a catalog");
				if !catalog.video.renditions.is_empty() {
					return catalog;
				}
			}
		})
		.await
		.expect("the canvas lists its video");
		(broadcast, catalogs, catalog)
	}

	/// Read a further canvas's `video` and `catalogs` to a clean end.
	async fn ended(mut catalogs: moq_mux::catalog::Consumer<()>, mut video: moq_net::track::Subscriber) {
		kio::clock::timeout(TIMEOUT, async {
			while let Some(mut group) = video.recv_group().await.expect("the video finishes") {
				while group.read_frame().await.expect("the video finishes").is_some() {}
			}
			while catalogs.next().await.expect("the catalog finishes").is_some() {}
		})
		.await
		.expect("the canvas's tracks end");
	}

	/// A further canvas reaches the relay in a broadcast of its own beneath the
	/// main one, at its own size, with the main broadcast's audio listed by
	/// reference rather than encoded again, and holds one more encode session.
	/// Removing it, or quitting, ends its video and then its catalog cleanly,
	/// then its broadcast; removing it gives its session back.
	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "needs a Vulkan device and an H.264 encoder; run by just rs render-sw"]
	async fn the_program_of_a_further_canvas_has_its_own_broadcast() {
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
		let gpu = Gpu::new().await.expect("a GPU");
		let (autosave, _) = Autosave::open(dir.path().join("show.toml")).unwrap();
		let relays = vec![Relay {
			client,
			url: fixture.url.clone(),
		}];
		let mut loaded = red_canvas();
		let tall = moq_scene::Extra::new(360, 640);
		let fill = moq_scene::Rect::fill(&tall.canvas(30));
		loaded.collection.canvases.insert("tall".into(), tall);
		loaded.collection.scenes[0]
			.canvases
			.insert("tall".into(), vec![moq_scene::Item::new("red", fill)]);
		let engine = Engine::start(launch(gpu, relays, dir.path()), autosave, loaded)
			.await
			.expect("the studio starts");

		let (broadcast, catalogs, catalog) = routed_tall(&relay).await;
		let (_, audio) = catalog.audio.renditions.iter().next().expect("the program audio");
		let main = audio.broadcast.as_ref().expect("a reference to the main broadcast");
		let main = moq_net::Path::new("studio/tall").unwrap().try_resolve(main);
		assert_eq!(main.as_ref().map(moq_net::Path::as_str), Some("studio"));
		assert!(audio.instance.is_some(), "the main broadcast's instance");
		let canvas_size = |catalog: &hang::Catalog| {
			let (video, config) = catalog
				.video
				.renditions
				.iter()
				.find(|(_, config)| config.coded_width == Some(360))
				.expect("the canvas size");
			assert_eq!(config.coded_height, Some(640));
			(video.clone(), config.clone())
		};
		let (video, config) = canvas_size(&catalog);
		let mut pictures = subscribe(&broadcast, &video, &config).await;
		red(&mut pictures, moq_video::Size::new(360, 640))
			.await
			.expect("the canvas's own layout arrives");
		assert_eq!(engine.stats.sessions.open(), 2, "one canvas size each");
		assert!(engine.stats.canvases()["tall"].error().is_none());

		let track = broadcast.track(&video).unwrap().subscribe(None).await.unwrap();
		{
			let mut state = engine.state.lock().unwrap();
			let mut draft = state.collection().draft("Remove the canvas");
			draft.remove_extra("tall").unwrap();
			state.commit(draft, None).unwrap();
		}
		ended(catalogs, track).await;
		kio::clock::timeout(TIMEOUT, broadcast.closed())
			.await
			.expect("the canvas's broadcast ends");
		assert!(engine.stats.canvases().is_empty());
		assert_eq!(engine.stats.sessions.open(), 1, "the canvas's session given back");

		// Back again, quitting ends it as cleanly.
		engine.state.lock().unwrap().undo().unwrap();
		let (broadcast, catalogs, catalog) = routed_tall(&relay).await;
		let (video, _) = canvas_size(&catalog);
		let track = broadcast.track(&video).unwrap().subscribe(None).await.unwrap();
		let stopping = engine.clone();
		tokio::task::spawn_blocking(move || stopping.stop()).await.unwrap();
		kio::clock::timeout(TIMEOUT, engine.close())
			.await
			.expect("the studio closes");
		ended(catalogs, track).await;
		kio::clock::timeout(TIMEOUT, broadcast.closed())
			.await
			.expect("the canvas's broadcast ends");
		assert_eq!(engine.stats.sessions.open(), 0);
	}

	/// A guest's return feed lists every size of `engine`'s program that
	/// `published` lists, read from the program as served at `.program`, which
	/// holds the program's tracks and none of the sources'.
	async fn returned(engine: &Engine, published: &hang::Catalog) {
		use crate::guests::{PROGRAM, Room, tests};

		let connect = url::Url::parse("https://relay.example/show").unwrap();
		let page = url::Url::parse("https://studio.example/join/").unwrap();
		let key = tests::key("show/guests", &["**"], &["**"]);
		let room = Arc::new(Room::new(&connect, "guests", key, page, tests::EXPIRY).unwrap());
		let camera = moq_net::Path::new("alice/camera.hang").unwrap();
		room.observe(&camera, true, &Collection::new(), &History::new());
		let origin = moq_tokio::origin::spawn();
		let program = engine.program.publication.broadcast().consume();
		let clock = engine.program.clock.clone();
		let mut feeds = crate::feed::Feeds::new(room, origin.clone(), program, clock, Duration::ZERO).unwrap();
		{
			let state = engine.state.lock().unwrap();
			feeds.sync(state.collection(), &state.sources);
		}

		let format = moq_mux::catalog::CatalogFormat::Hang;
		let feed = Room::feed_path("alice", 0).unwrap();
		let broadcast = origin.consume().follow(feed, None).unwrap().available().await.unwrap();
		let mut listings = moq_mux::catalog::Consumer::<()>::new(&broadcast, format).await.unwrap();
		let listed = kio::clock::timeout(TIMEOUT, async {
			loop {
				let catalog = listings.next().await.unwrap().expect("a catalog");
				if !catalog.video.renditions.is_empty() {
					return catalog;
				}
			}
		})
		.await
		.expect("the feed lists the program's video");
		let names = |video: &hang::catalog::Video| video.renditions.keys().cloned().collect::<Vec<_>>();
		assert_eq!(names(&listed.video), names(&published.video), "both sizes");
		for config in listed.video.renditions.values() {
			assert_eq!(config.broadcast.as_ref().unwrap().as_str(), "../../.program");
		}

		let served = origin.consume().request(PROGRAM, None).await.unwrap();
		let mut served = moq_mux::catalog::Consumer::<moq_scene::publish::Ext>::new(&served, format)
			.await
			.unwrap();
		let served = served.next().await.unwrap().expect("a catalog");
		assert_eq!(names(&served.video), names(&published.video));
		assert!(served.ext.studio.sources.is_empty(), "a source reached the guests");
	}

	/// The program a studio publishes reaches a relay as the picture it composed,
	/// at the canvas size and at half of it, with its audio alongside. The half
	/// size holds an encode session only once watched.
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
		let gpu = Gpu::new().await.expect("a GPU");
		let (autosave, _) = Autosave::open(dir.path().join("show.toml")).unwrap();
		let relays = vec![Relay {
			client,
			url: fixture.url.clone(),
		}];
		let engine = Engine::start(launch(gpu, relays, dir.path()), autosave, red_canvas())
			.await
			.expect("the studio starts");
		assert!(
			engine.stats.video.encoder().is_some(),
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
			.iter()
			.map(|(name, config)| (config.coded_width, config.coded_height, name.clone(), config.clone()))
			.collect();
		sizes.sort_unstable_by_key(|size| std::cmp::Reverse(size.0));
		let [
			(Some(640), Some(360), video, config),
			(Some(320), Some(180), half, half_config),
		] = &sizes[..]
		else {
			panic!("the program at both sizes, and only it: {sizes:?}");
		};
		let video = video.clone();
		// Viewers are asked for feedback beneath the broadcast from its first catalog.
		let echo = catalog.echo.as_ref().expect("the catalog solicits feedback");
		assert_eq!(echo.path.as_str(), "studio/viewers");
		assert_eq!(engine.stats.sessions.open(), 1, "the half size is not watched yet");
		returned(&engine, &catalog).await;
		let mut pictures = subscribe(&broadcast, &video, config).await;
		let shown = red(&mut pictures, moq_video::Size::new(640, 360))
			.await
			.expect("the composed picture arrives");
		let mut small = subscribe(&broadcast, half, half_config).await;
		red(&mut small, moq_video::Size::new(320, 180))
			.await
			.expect("the half size arrives once watched");
		assert_eq!(engine.stats.sessions.open(), 2);
		drop(small);

		// The program reaches the encoder as a GPU surface when the encoder
		// reads one, and is read back frame by frame only when it does not.
		let stats = &engine.stats.video;
		for load in &stats.renditions[..2] {
			assert!(load.copied.load(Ordering::Relaxed) > 0);
			let read_back = load.read_back.load(Ordering::Relaxed);
			if stats.zero_copy.load(Ordering::Relaxed) {
				assert_eq!(read_back, 0, "a zero-copy program was read back");
			} else {
				assert!(read_back > 0, "the program was neither exported nor read back");
			}
		}

		// The color source keeps its still in the studio's own broadcast, not
		// the published one.
		let output = &engine.program;
		kio::clock::timeout(TIMEOUT, async {
			while !output.sources.catalog().ext.studio.sources.contains_key("red") {
				kio::clock::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("the source is listed locally");
		let mut published = moq_mux::catalog::Consumer::<moq_scene::publish::Ext>::new(&broadcast, format)
			.await
			.expect("catalog");
		let latest = published.next().await.expect("catalog").expect("a catalog");
		assert!(latest.ext.studio.sources.is_empty(), "no source reaches the relay");

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
					&& **name != video
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
		// Too small to halve, so one rendition, and one session.
		assert_eq!(engine.stats.sessions.open(), 1);
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

		// A recording beside the live program has its own encoder, a session
		// beside the canvas size's.
		let frames = recorded(&engine);
		engine.recorder.start(true).expect("recording starts");
		encoded(&engine, frames + 60).await;
		assert_eq!(engine.stats.sessions.open(), 2);
		let files = finished(&engine.recorder).await;
		assert_eq!(files, [["soun", "vide"]]);
		assert!(engine.stats.error().is_none());

		// The program's recent past saves as a clip, from the groups it holds.
		let take = crate::replay::take(&engine, &crate::replay::Of::Program, Duration::from_secs(1), false)
			.expect("the program's past");
		assert!(take.video.is_some(), "the canvas size holds video");
		let clip = kio::clock::timeout(TIMEOUT, crate::replay::save(take, engine.autosave.path()))
			.await
			.expect("the clip saves")
			.expect("a clip");
		assert_eq!(clip, dir.path().join("show-clip-1.mp4"));

		// Quitting ends the program's video and audio cleanly once the encoders
		// published what they were handed, and closes the relay session only
		// once the relay has it, rather than leaving viewers dropped tracks.
		let video = broadcast.track(&resized).unwrap().subscribe(None).await.unwrap();
		let sound = broadcast.track(audio).unwrap().subscribe(None).await.unwrap();
		let stopping = engine.clone();
		tokio::task::spawn_blocking(move || stopping.stop()).await.unwrap();
		kio::clock::timeout(TIMEOUT, engine.close())
			.await
			.expect("the studio closes");
		for mut track in [video, sound] {
			kio::clock::timeout(TIMEOUT, async {
				while let Some(mut group) = track.recv_group().await.expect("the track finishes") {
					while group.read_frame().await.expect("the track finishes").is_some() {}
				}
			})
			.await
			.expect("the track ends");
		}
		let failed = engine.stats.error();
		assert!(failed.is_none(), "{failed:?}");
	}

	/// With a failover relay, both relays route the program under its one
	/// instance over the one protocol version, so a viewer moving between them
	/// resumes. A broadcast source only the failover has still reaches the
	/// studio, and one restarted behind the failover replaces the first
	/// relay's once that one ends.
	#[tokio::test(flavor = "multi_thread")]
	async fn every_relay_routes_one_instance() {
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
		let instance = moq_net::Instance::random();
		publication
			.announce(moq_net::origin::Route::default().with_instance(instance.clone(), 0))
			.unwrap();

		// Publish `path` under `instance` through one relay's guest.
		let publish = |path: &str, instance: &moq_net::Instance, (client, url): &(moq_tokio::Client, url::Url)| {
			let origin = moq_tokio::origin::spawn();
			let source = origin.create_broadcast(path).unwrap();
			source
				.announce(moq_net::origin::Route::default().with_instance(instance.clone(), 0))
				.unwrap();
			let connection = client.clone().with_publisher(&origin).connect(url.clone());
			(origin, source, connection)
		};
		let _guest = publish("guest", &moq_net::Instance::random(), &guests[1]);
		let mut sources = subscribe.consume().announced();

		let routed = async {
			for relay in &routes {
				let mut announced = relay.announced();
				loop {
					match announced.next().await.expect("announcements") {
						moq_net::announce::Event::Start(announce) if announce.prefix.as_str() == "studio" => {
							assert_eq!(announce.route.instance, Some(instance.clone()));
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

			// A source restarted behind the failover takes over only once the
			// first relay's route to it ends, since that relay ranks first.
			let older = moq_net::Instance::random();
			let newer = moq_net::Instance::random();
			let first = publish("show", &older, &guests[0]);
			routed_at(&mut sources, &older).await;
			let _restarted = publish("show", &newer, &guests[1]);
			routed_at(&mut routes[1].announced(), &newer).await;
			// Announced after the restart on the failover's one announce
			// stream, so once it reaches the studio the restart has too.
			let _marker = publish("marker", &moq_net::Instance::random(), &guests[1]);
			subscribe
				.consume()
				.follow("marker", None)
				.unwrap()
				.available()
				.await
				.expect("the marker");
			while let Some(event) = sources.try_next() {
				if let moq_net::announce::Event::Start(announce)
				| moq_net::announce::Event::Update(announce)
				| moq_net::announce::Event::Restart(announce)
				| moq_net::announce::Event::Takeover { announce, .. } = event
				{
					assert!(
						announce.prefix.as_str() != "show" || announce.route.instance.as_ref() != Some(&newer),
						"the failover's instance won while the first relay has one"
					);
				}
			}
			drop(first);
			routed_at(&mut sources, &newer).await;
		};
		kio::clock::timeout(TIMEOUT, routed)
			.await
			.expect("both relays route the program, the failover the sources");
	}

	/// Closing the output as quitting does delivers the tail of every finished
	/// track before the relay session goes: a subscriber at the relay reads
	/// each track's last group whole and then its end, and the broadcast is
	/// withdrawn. Each last group is still on its way as the close starts, so
	/// dropping the session instead cuts it.
	#[tokio::test(start_paused = true)]
	async fn closing_delivers_every_tail() {
		use moq_net::announce::Event;

		/// The last group's frames, each padded to this many bytes.
		const FRAMES: usize = 10;
		const PADDING: usize = 50_000;

		let _ = moq_tokio::crypto::install_default();
		let fixture = moq_relay::test_relay().await.expect("test relay");
		let ready = fixture.relay.ready();
		let relay = fixture.relay.cluster().origin.consume();
		tokio::spawn(fixture.relay.run());
		ready.wait().await.expect("relay ready");
		let mut client = moq_tokio::connect::Config::default();
		client.tls.fingerprint = vec![fixture.fingerprint.clone()];
		let client = client.init(Default::default()).expect("client");

		let origin = moq_tokio::origin::spawn();
		let (connections, _links) = connect(
			vec![Relay {
				client: client.clone(),
				url: fixture.url.clone(),
			}],
			&origin,
			&moq_tokio::origin::spawn(),
		);
		let output = Output {
			path: "studio".into(),
			on_demand: false,
			connections,
			clients: vec![client],
		};
		let publication = origin.create_broadcast("studio").unwrap();
		publication.announce(Default::default()).unwrap();
		let info = || moq_net::track::Info::default().with_timescale(moq_net::Timescale::MILLI);
		let tracks = ["video", "audio"].map(|name| publication.create_track(name, info()).unwrap());

		// Wait for an event `want` picks on `announced`.
		let routed = async |announced: &mut moq_net::announce::Consumer, want: fn(&Event) -> bool| {
			kio::clock::timeout(TIMEOUT, async {
				while !want(&announced.next().await.expect("announcements")) {}
			})
			.await
		};
		let mut announced = relay.announced();
		routed(
			&mut announced,
			|event| matches!(event, Event::Start(announce) if announce.prefix.as_str() == "studio"),
		)
		.await
		.expect("the relay routes the broadcast");
		let broadcast = kio::clock::timeout(TIMEOUT, relay.follow("studio", None).unwrap().available())
			.await
			.expect("the relay routes the broadcast")
			.expect("broadcast");
		let mut subscriptions = Vec::new();
		for (name, track) in ["video", "audio"].into_iter().zip(&tracks) {
			let mut subscription = broadcast.track(name).unwrap().subscribe(None).await.unwrap();
			// A head group read through the relay, so only the tail is at stake.
			let mut group = track.append_group().unwrap();
			group.write_frame(moq_net::Timestamp::ZERO, vec![0; 1]).unwrap();
			group.finish().unwrap();
			let mut head = kio::clock::timeout(TIMEOUT, subscription.recv_group())
				.await
				.expect("the head group arrives")
				.unwrap()
				.expect("a group");
			while head.read_frame().await.unwrap().is_some() {}
			subscriptions.push(subscription);
		}

		// Read as a viewer does, while the studio writes and closes.
		let readers: Vec<_> = subscriptions
			.into_iter()
			.map(|mut subscription| {
				tokio::spawn(async move {
					let mut tail = subscription
						.recv_group()
						.await
						.expect("the tail, not a cut")
						.expect("a group");
					let mut frames = 0;
					while let Some(frame) = tail.read_frame().await.expect("the whole tail") {
						assert_eq!(frame.payload.len(), PADDING);
						frames += 1;
					}
					assert_eq!(frames, FRAMES);
					let end = subscription.recv_group().await.expect("a clean end");
					assert!(end.is_none(), "the track ends after its tail");
				})
			})
			.collect();
		for track in &tracks {
			let mut group = track.append_group().unwrap();
			for _ in 0..FRAMES {
				group.write_frame(moq_net::Timestamp::ZERO, vec![0; PADDING]).unwrap();
			}
			group.finish().unwrap();
			track.finish().unwrap();
		}
		kio::clock::timeout(TIMEOUT, output.close())
			.await
			.expect("the output closes");
		for reader in readers {
			kio::clock::timeout(TIMEOUT, reader)
				.await
				.expect("the tail arrives")
				.unwrap();
		}
		routed(
			&mut announced,
			|event| matches!(event, Event::End(announce) if announce.prefix.as_str() == "studio"),
		)
		.await
		.expect("the broadcast is withdrawn");
	}

	/// Wait until `announced` routes `show` under `instance`.
	async fn routed_at(announced: &mut moq_net::announce::Consumer, instance: &moq_net::Instance) {
		use moq_net::announce::Event;
		loop {
			let announce = match announced.next().await.expect("announcements") {
				Event::Start(announce) | Event::Update(announce) | Event::Restart(announce) => announce,
				Event::Takeover { announce, .. } => announce,
				Event::End(_) => continue,
			};
			if announce.prefix.as_str() == "show" && announce.route.instance.as_ref() == Some(instance) {
				return;
			}
		}
	}
}
