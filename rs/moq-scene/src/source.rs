//! Live sources: what each [`Source`] of a collection shows and plays right now.
//!
//! [`Sources::sync`] keeps one [`Feed`] per source and hands back a [`Driver`]
//! for each one it opens. The caller runs the drivers wherever suits it (a
//! capture driver is `!Send` on macOS); a driver stops once its feed is
//! dropped, which happens when the source is removed or edited.
//!
//! With [`Context::publish`] set, each local source that shows a picture,
//! and each microphone and system audio, also publishes it as its own track in
//! the studio's local broadcast, web pages and overlays aside; see
//! [`crate::publish`].

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hang::catalog::{AudioConfig, Reference, VideoConfig};
use kio::clock::Instant;
use moq_audio::bus::{Bus, Mix, Strip};
use moq_mux::catalog::Stream;
use moq_mux::timeline;
use tokio::sync::{Notify, watch};

use crate::Error;
use crate::browser::Page;
use crate::collection::{Collection, Source};
use crate::lut::Cube;
use crate::media::Clip;
use crate::playout::{Follow, Place, Speaker, VIDEO_DELAY, WRITE_LEAD, Writer, follow};
pub use crate::playout::{MAX_DELAY, heard};
use crate::publish::{Claim, Listing, Publish, Still};
use crate::replay::{MIN_SPEED, Take, Target};

/// A source's newest picture, waiting for the compositor to draw it.
pub(crate) enum Picture {
	/// A captured or decoded video frame.
	Frame(moq_video::Frame),
	/// Tightly packed RGBA with straight alpha, such as a still image.
	Rgba { size: moq_video::Size, pixels: Vec<u8> },
	/// A texture on the compositor's device with premultiplied alpha, such as
	/// a web page.
	#[cfg_attr(
		not(target_os = "linux"),
		expect(dead_code, reason = "browser sources are Linux-only")
	)]
	Texture(moq_video::render::wgpu::Texture),
	/// A LUT source's table, which is not drawn but looked up.
	Lut(Cube),
	/// An overlay's HTML and assets, which the compositor lays out and paints
	/// itself, on the program clock.
	Overlay(crate::overlay::Content),
}

/// The one picture a source holds for the compositor, and whether it cleared
/// its picture since the compositor last took one. A newer picture replaces an
/// undrawn one, so a slow canvas skips pictures instead of queuing them, but
/// not the clear before it.
#[derive(Default)]
pub(crate) struct Slot {
	picture: Option<Picture>,
	cleared: bool,
}

impl Slot {
	/// Hold `picture` for the compositor, replacing one not yet drawn.
	pub fn show(&mut self, picture: Picture) {
		self.picture = Some(picture);
	}

	/// Show nothing, ending what the source showed before, such as a play
	/// stopped or a page closed.
	pub fn clear(&mut self) {
		self.picture = Some(clear());
		self.cleared = true;
	}

	fn take(&mut self) -> Option<Taken> {
		let picture = self.picture.take()?;
		Some(Taken {
			picture,
			cleared: std::mem::take(&mut self.cleared),
		})
	}
}

pub(crate) type Mailbox = Arc<Mutex<Slot>>;

/// A picture the compositor takes from a source.
pub(crate) struct Taken {
	pub picture: Picture,
	/// Whether the source cleared its picture since the last one taken, even
	/// if `picture` came after the clear: nothing from before it may show.
	pub cleared: bool,
}

/// When a source last delivered a picture or, for a microphone or system
/// audio, a buffer of sound. `None` before its first.
type Delivered = Arc<Mutex<Option<Instant>>>;

/// What a broadcast source plays, for a replay of it to copy.
#[derive(Default)]
struct Played {
	/// The instance of the broadcast online now.
	broadcast: Option<moq_net::broadcast::Consumer>,
	video: Option<(String, VideoConfig)>,
	audio: Option<(String, AudioConfig)>,
	/// The newest picture shown from it, where a replay of it ends.
	newest: Option<moq_net::Timestamp>,
}

type Playback = Arc<Mutex<Played>>;

/// A take for a replay source to play, how fast, and which play it is, as
/// [`Run::play`] counts them.
struct Play {
	take: Take,
	speed: f32,
	play: u64,
}

/// A replay source's next replay, until its driver takes it, so a replay that
/// ended holds its take no longer.
type Next = Arc<Mutex<Option<Play>>>;

/// A media or replay source's latest play: which it is, and when it began and
/// ended on the program clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Run {
	/// Which play it is: 0 for the one the source opens with, in which a
	/// replay source plays nothing, and one more for each [`Feed::play`] or
	/// [`Feed::replay`] since.
	pub play: u64,
	/// When it begins, or `None` until its first picture and sound are
	/// decoded: a media file's earlier of the two shows or sounds then, and a
	/// replay's span starts then, even where it starts in a gap. Set a little
	/// ahead: the bus sounds a write 50 ms later, so a play begins that long
	/// after it is decoded.
	pub begun: Option<Instant>,
	/// When its last picture or sound finishes, a looping one's first pass
	/// does, a replay's span does, or [`Feed::stop`] stopped it, or `None`
	/// until known. A looping one's is known 50 ms ahead. A play cut off by
	/// the next never ends.
	pub ended: Option<Instant>,
}

/// What a media source's driver is asked to do, which the feed sends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Cue {
	/// The play, as [`Run::play`] counts them.
	play: u64,
	/// Whether it is stopped, blank and silent until the next play.
	stopped: bool,
}

/// A media source's latest [`Run`], which the feed starts and only the driver
/// playing that one writes, and whether the feed stopped it.
#[derive(Default)]
struct Latest {
	run: Run,
	stopped: bool,
}

/// The feed changes it, and blanks the picture, under its lock, which the
/// driver takes to show a picture, so a play cut off never shows over that.
type Runs = Arc<Mutex<Latest>>;

/// A picture that shows nothing, for a replay source between replays.
fn clear() -> Picture {
	Picture::Rgba {
		size: moq_video::Size::new(1, 1),
		pixels: vec![0; 4],
	}
}

/// How long a camera, broadcast, microphone, or system audio may go without
/// delivering before it reads as stalled. Each delivers several pictures or
/// buffers a second while it works, so a second without one is a freeze a
/// viewer sees or a silence they hear.
pub const STALL: Duration = Duration::from_secs(1);

/// What a live source stopped delivering, and for how long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stall {
	/// A camera or broadcast without a new picture: the program repeats its last one.
	Picture(Duration),
	/// A microphone or system audio without new samples: the program plays silence for it.
	Sound(Duration),
}

impl Stall {
	/// How long the source has gone without delivering.
	pub fn since(&self) -> Duration {
		match self {
			Self::Picture(since) | Self::Sound(since) => *since,
		}
	}
}

/// One instance of a broadcast being decoded.
type Playing<'a> = Pin<Box<dyn Future<Output = Result<(), Error>> + 'a>>;

/// The playout clock one instance of a broadcast's audio and video share.
struct Clock {
	timeline: timeline::Producer,
	/// Woken when audio places a frame or stops, which may move the anchor or
	/// let a picture waiting for its sound show.
	changed: Notify,
}

/// Decoded pictures a broadcast source holds until they are due. The rest wait
/// undecoded in the subscription, as in `moq play`, so a deep playout target
/// holds no more decoder output than this.
const QUEUED: usize = 3;

/// What a running source is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Status {
	/// Opening its device, file, or subscription.
	Starting,
	/// Producing pictures or sound.
	Live,
	/// Waiting for its broadcast to be announced.
	Offline,
	/// Stopped, with the reason. A broadcast source keeps waiting for the
	/// broadcast to come back; every other source stays stopped until edited
	/// or [restarted](Sources::restart).
	Failed(String),
	/// Played to its end: a media file that does not loop, holding its last
	/// picture, silent, until [played](Feed::play) again.
	Ended,
}

/// What every source shares.
#[derive(Clone)]
pub struct Context {
	/// Where microphones, system audio, and broadcasts feed their sound, and
	/// where each of the collection's audio scenes is mixed.
	pub bus: Bus,
	/// How far `bus`'s [`Mixer`] has mixed it, on which broadcast, media, and
	/// replay sources place their sound.
	pub mixed: Mixed,
	/// Where broadcast sources are announced: the studio's relay connection.
	pub origin: moq_net::origin::Consumer,
	/// What a relative image, LUT, or overlay asset path is relative to: the
	/// scene file's directory.
	pub dir: PathBuf,
	/// What cameras, screens, and broadcasts deliver video as:
	/// `moq_video::render::output` of the compositor's device.
	pub output: moq_video::Output,
	/// What browser sources run in; without it they fail as unsupported.
	pub browser: Option<crate::browser::Helper>,
	/// Where local sources publish their own tracks, or `None` to publish nothing.
	pub publish: Option<Publish>,
}

/// Advances a bus sample-exact on the clock from its start, as the clock's
/// owner moves it, telling the sources how far it has mixed.
pub struct Mixer {
	driver: moq_audio::bus::Driver,
	position: watch::Sender<u64>,
}

impl Mixer {
	/// Advance `driver`'s bus, which mixes its position 0 at `start`, and
	/// follow it with the returned [`Mixed`].
	pub fn new(driver: moq_audio::bus::Driver, start: Instant) -> (Self, Mixed) {
		let (position, follow) = watch::channel(driver.position());
		let mixed = Mixed {
			start,
			position: follow,
		};
		(Self { driver, position }, mixed)
	}

	/// Mix the next `frames` of every mix, as the clock moves by that much.
	pub fn advance(&mut self, frames: usize) {
		// Under the position's lock, so no source writes in between reading
		// it and the bus moving past it.
		self.position.send_modify(|position| {
			self.driver.advance(frames);
			*position = self.driver.position();
		});
	}
}

/// How far a [`Mixer`] has mixed its bus, and when it mixes position 0.
#[derive(Clone)]
pub struct Mixed {
	pub(crate) start: Instant,
	/// The bus position of the next frame mixed, which cannot move while borrowed.
	pub(crate) position: watch::Receiver<u64>,
}

impl Mixed {
	/// When the bus mixes its position 0, so a sound due at `t` plays at bus
	/// position `(t - start) * rate`.
	pub fn start(&self) -> Instant {
		self.start
	}
}

/// The running sources of a collection, and the mix of each audio scene.
pub struct Sources {
	context: Context,
	feeds: BTreeMap<String, Feed>,
	/// Each audio scene's mix, by its name.
	mixes: BTreeMap<String, Mix>,
	/// The sources on program, as last set.
	on_program: BTreeSet<String>,
}

impl Sources {
	/// No sources or mixes yet; [`sync`](Self::sync) starts them.
	pub fn new(context: Context) -> Self {
		Self {
			context,
			feeds: BTreeMap::new(),
			mixes: BTreeMap::new(),
			on_program: BTreeSet::new(),
		}
	}

	/// Note the sources on program now, by name, such as those a scene on air
	/// shows. A media source set to restart plays from its start as it goes on
	/// program.
	pub fn set_on_program(&mut self, on_program: &BTreeSet<String>) {
		if *on_program == self.on_program {
			return;
		}
		for name in on_program.difference(&self.on_program) {
			let Some(feed) = self.feeds.get(name) else { continue };
			if matches!(feed.source, Source::Media { restart: true, .. })
				&& let Err(err) = feed.play()
			{
				tracing::warn!(source = %name, %err, "cannot restart the source");
			}
		}
		self.on_program.clone_from(on_program);
	}

	/// What every source shares.
	pub fn context(&self) -> &Context {
		&self.context
	}

	/// Run exactly the sources and audio scenes `collection` has: start new and edited
	/// sources and stop removed and edited ones. A clone shares the feed of the
	/// source it clones rather than opening one. Returns a driver for each
	/// source started, which does nothing until run. The media source a
	/// stinger plays opens silent and blank, waiting for its first
	/// [play](Feed::play), rather than playing once at the start.
	///
	/// Also applies the collection's gain, mute, pan, and delay to every audio
	/// strip, routes each into the mix of every audio scene that reaches it,
	/// and applies the frame rate to every web page, which never restarts a
	/// source. A removed or edited source leaves every mix at once, so its
	/// replacement never plays over it. Panics on a gain, pan, or delay
	/// [`Collection::validate`] refuses.
	pub fn sync(&mut self, collection: &Collection) -> Vec<Driver> {
		let mixes = &self.mixes;
		self.feeds.retain(|name, feed| {
			let keep = collection.sources.get(name) == Some(&feed.source);
			if !keep {
				leave(mixes, feed);
			}
			keep
		});
		// A layer plays on its broadcast source's clock, so it stops with it
		// and starts again on the replacement's.
		let orphaned: Vec<String> = self
			.feeds
			.iter()
			.filter(
				|(_, feed)| matches!(&feed.source, Source::Layer { broadcast, .. } if !self.feeds.contains_key(broadcast)),
			)
			.map(|(name, _)| name.clone())
			.collect();
		for name in orphaned {
			let feed = self.feeds.remove(&name).expect("listed above");
			leave(&self.mixes, &feed);
		}
		// A mix ends once its last handle drops, which may be a reader's.
		self.mixes.retain(|name, _| collection.audio_scene(name).is_some());
		for scene in &collection.audio_scenes {
			if !self.mixes.contains_key(&scene.name) {
				self.mixes.insert(scene.name.clone(), self.context.bus.mix());
			}
		}
		// Nested audio scenes are flattened: a mix sums every source an audio
		// scene reaches, each once, so a sound reached two ways is not doubled.
		let heard: BTreeMap<&str, BTreeSet<&str>> = collection
			.audio_scenes
			.iter()
			.map(|scene| (scene.name.as_str(), collection.heard(&scene.name)))
			.collect();

		let mut drivers = Vec::new();
		// Layers last, once the broadcast sources whose clocks they play on run.
		let (layers, opens): (Vec<_>, Vec<_>) = collection
			.sources
			.iter()
			.filter(|(_, source)| source.opens())
			.partition(|(_, source)| matches!(source, Source::Layer { .. }));
		for (name, source) in opens.into_iter().chain(layers) {
			if !self.feeds.contains_key(name) {
				let held = collection.transition.stinger() == Some(name.as_str());
				let follows = match source {
					Source::Layer { broadcast, .. } => match collection.sources.get(broadcast) {
						Some(Source::Broadcast { path }) => {
							self.feeds[broadcast].clock.clone().map(|clock| (path.clone(), clock))
						}
						_ => None,
					},
					_ => None,
				};
				let (feed, driver) = open(name, source, &self.context, held, follows);
				self.feeds.insert(name.clone(), feed);
				drivers.push(driver);
			}
			// A page paints at the canvas's frame rate, live.
			if let Some(page) = self.feeds[name].page() {
				page.set_framerate(collection.canvas.framerate);
			}

			if let Some(strip) = self.feeds[name].strip() {
				let audio = collection.audio.get(name).cloned().unwrap_or_default();
				strip.set_gain(audio.gain);
				strip.set_muted(audio.muted);
				// The bus is stereo, the one layout that refuses no pan.
				strip.set_pan(audio.pan).expect("stereo bus pans");
				strip.set_delay(audio.delay);
				for (scene, mix) in &self.mixes {
					// A source published independently reaches viewers by its own
					// track, never through a mix.
					let routed = !collection.published.contains(name)
						&& heard
							.get(scene.as_str())
							.is_some_and(|heard| heard.contains(name.as_str()));
					if mix.routes(strip) == routed {
						continue;
					}
					if routed {
						mix.route(strip);
					} else {
						mix.unroute(strip);
					}
				}
			}
		}
		drivers
	}

	/// Stop the source called `name`, so the next [`sync`](Self::sync) starts
	/// it again, as an edit would, such as to reload a web page. It leaves
	/// every mix at once, as an edited source does.
	pub fn restart(&mut self, name: &str) {
		if let Some(feed) = self.feeds.remove(name) {
			leave(&self.mixes, &feed);
		}
	}

	/// Stop every source, as removing it would: each [`Driver`] returns and
	/// its own track finishes cleanly with what it published. The mixes stay,
	/// and the next [`sync`](Self::sync) starts the sources again.
	pub fn stop(&mut self) {
		for feed in std::mem::take(&mut self.feeds).into_values() {
			leave(&self.mixes, &feed);
		}
	}

	/// The running source called `name`.
	pub fn get(&self, name: &str) -> Option<&Feed> {
		self.feeds.get(name)
	}

	/// The mix of the audio scene called `scene`, as the collection last
	/// synced had it.
	pub fn mix(&self, scene: &str) -> Option<&Mix> {
		self.mixes.get(scene)
	}

	/// Whether the audio scene called `scene` plays the source called `name`:
	/// it has sound, is not muted, and the audio scene reaches it.
	pub fn plays(&self, scene: &str, name: &str) -> bool {
		let strip = self.feeds.get(name).and_then(Feed::strip);
		let mix = self.mixes.get(scene);
		strip
			.zip(mix)
			.is_some_and(|(strip, mix)| !strip.muted() && mix.routes(strip))
	}

	/// Every running source, by name.
	pub fn iter(&self) -> impl Iterator<Item = (&str, &Feed)> {
		self.feeds.iter().map(|(name, feed)| (name.as_str(), feed))
	}
}

/// Take a stopping `feed` out of every one of `mixes`, so it leaves nothing
/// behind in them, nor plays over a replacement.
fn leave(mixes: &BTreeMap<String, Mix>, feed: &Feed) {
	let Some(strip) = &feed.strip else { return };
	for mix in mixes.values() {
		mix.unroute(strip);
	}
}

/// One running source. Dropping it stops its [`Driver`].
pub struct Feed {
	source: Source,
	picture: Mailbox,
	delivered: Delivered,
	strip: Option<Strip>,
	status: kio::Consumer<Status>,
	page: Option<Page>,
	track: Option<kio::Consumer<Status>>,
	played: Playback,
	/// A replay source's next replay.
	plays: Option<watch::Sender<Next>>,
	/// Sent each play or stop a media source is asked for.
	cues: Option<watch::Sender<Cue>>,
	/// A media source's latest play.
	runs: Runs,
	/// Set once a media source's file turns out to hold no sound.
	silent: Arc<AtomicBool>,
	/// A broadcast source's playout clock, or the one a layer plays on.
	clock: Option<timeline::Consumer>,
}

impl Feed {
	/// What a replay of this broadcast source copies: the broadcast it plays,
	/// up to the newest picture it showed, which is what was just seen.
	///
	/// `None` until a broadcast source shows a picture, and for every other
	/// kind of source, whose recent past is on its own track if anywhere.
	pub fn target(&self) -> Option<Target> {
		let played = self.played.lock().unwrap();
		Some(Target {
			broadcast: played.broadcast.clone()?,
			video: played.video.clone(),
			audio: played.audio.clone(),
			end: played.newest?,
		})
	}

	/// Play `take` at `speed`, from [`MIN_SPEED`] to 1 for real time, in place
	/// of whatever this replay source plays now. Its sound plays only in real
	/// time.
	///
	/// Refused by any other kind of source, by a replay source that stopped,
	/// and for a take of sound alone, slowed down, which would play nothing.
	pub fn replay(&self, take: Take, speed: f32) -> Result<(), Error> {
		let Some(plays) = &self.plays else {
			return Err(Error::Unsupported("only a replay source plays replays".into()));
		};
		if !(MIN_SPEED..=1.0).contains(&speed) {
			return Err(Error::Invalid(format!(
				"a replay plays at {MIN_SPEED} to 1 times real time, not {speed}"
			)));
		}
		if take.video.is_none() && speed != 1.0 {
			return Err(Error::Unsupported(
				"a replay of sound alone plays only in real time".into(),
			));
		}
		if plays.is_closed() {
			return Err(self.stopped());
		}
		// Counted before the driver hears of it, so whatever the replay before
		// still reports or shows is not taken for this one's.
		let mut latest = self.runs.lock().unwrap();
		*latest = Latest {
			run: Run {
				play: latest.run.play + 1,
				..Run::default()
			},
			stopped: false,
		};
		// Cleared before the driver hears of it, so the replay cut off leaves
		// nothing up until the new one's first picture.
		self.picture.lock().unwrap().clear();
		let play = Play {
			take,
			speed,
			play: latest.run.play,
		};
		plays.send(Arc::new(Mutex::new(Some(play)))).map_err(|_| self.stopped())
	}

	/// Play a media source from its start, now, cutting off wherever it is.
	/// One that ended or stopped plays again, and one that loops loops from
	/// there.
	///
	/// Refused by any other kind of source, and by a media source whose driver
	/// is gone, such as after it failed.
	pub fn play(&self) -> Result<(), Error> {
		let Some(cues) = &self.cues else {
			return Err(Error::Unsupported("only a media source plays from its start".into()));
		};
		if cues.is_closed() {
			return Err(self.stopped());
		}
		// Counted before the driver hears of it, so whatever the play before
		// still reports is not taken for this one's.
		let mut latest = self.runs.lock().unwrap();
		*latest = Latest {
			run: Run {
				play: latest.run.play + 1,
				..Run::default()
			},
			stopped: false,
		};
		// Cleared before the driver hears of it, so the next frame drawn, such
		// as the first of a scene going on program, shows nothing rather than
		// where the last play was, and the new first picture replaces it.
		self.picture.lock().unwrap().clear();
		let cue = Cue {
			play: latest.run.play,
			stopped: false,
		};
		cues.send(cue).map_err(|_| self.stopped())
	}

	/// Stop a media source now: blank and silent, dropping any sound it has
	/// queued, until the next [`play`](Self::play). The play under way ends
	/// now.
	///
	/// Refused by any other kind of source.
	pub fn stop(&self) -> Result<(), Error> {
		let Some(cues) = &self.cues else {
			return Err(Error::Unsupported("only a media source stops".into()));
		};
		let mut latest = self.runs.lock().unwrap();
		latest.run.ended.get_or_insert_with(kio::clock::now);
		latest.stopped = true;
		self.picture.lock().unwrap().clear();
		// Kept even with the driver gone, which plays nothing anyway.
		cues.send_replace(Cue {
			play: latest.run.play,
			stopped: true,
		});
		Ok(())
	}

	/// A media source's latest play, from its start or the latest
	/// [`play`](Self::play), or a replay source's latest
	/// [`replay`](Self::replay). Never begins for any other source.
	pub fn run(&self) -> Run {
		self.runs.lock().unwrap().run
	}

	/// Why the driver is gone, from the reason it left in the status.
	fn stopped(&self) -> Error {
		Error::Stopped(match self.status() {
			Status::Failed(reason) => reason,
			status => format!("{status:?}"),
		})
	}

	/// What this feed was opened for.
	pub fn source(&self) -> &Source {
		&self.source
	}

	/// What the source is doing.
	pub fn status(&self) -> Status {
		self.status.read().clone()
	}

	/// What the source's own track is doing, or `None` when it publishes none:
	/// nothing is published, or the source is another broadcast, a web page,
	/// an overlay, or a LUT.
	///
	/// A failed track leaves the source drawing and playing in the studio's
	/// own scenes and audio scenes.
	pub fn track(&self) -> Option<Status> {
		self.track.as_ref().map(|track| track.read().clone())
	}

	/// What a live camera, broadcast, microphone, or system audio has gone
	/// without delivering, once that is at least [`STALL`] at `now`.
	///
	/// Displays and windows are never stalled: a capture of a still screen
	/// delivers nothing until it changes. Neither are sources that have not
	/// delivered yet, such as an audio-only broadcast: a broadcast stalls by its
	/// picture alone.
	pub fn stalled(&self, now: Instant) -> Option<Stall> {
		let stall = match self.source {
			Source::Camera { .. } | Source::Broadcast { .. } | Source::Layer { .. } => Stall::Picture,
			Source::Microphone { .. } | Source::SystemAudio => Stall::Sound,
			_ => return None,
		};
		if self.status() != Status::Live {
			return None;
		}
		let delivered = (*self.delivered.lock().unwrap())?;
		let since = now.saturating_duration_since(delivered);
		(since >= STALL).then(|| stall(since))
	}

	/// How far a live broadcast source plays behind what arrives, to ride out
	/// network jitter: its audio's playout target, which video follows, or
	/// without audio, video's own short delay. `None` for any other source.
	pub fn delay(&self) -> Option<Duration> {
		let clock = self.clock.as_ref()?;
		(self.status() == Status::Live).then(|| clock.target().unwrap_or(VIDEO_DELAY))
	}

	/// When a broadcast source plays what its broadcast stamps, or a layer
	/// what its broadcast source's does, so what else that broadcast carries
	/// can play on the same clock. `None` for any other source.
	pub fn clock(&self) -> Option<&timeline::Consumer> {
		self.clock.as_ref()
	}

	/// The source's gain, mute, delay, and meter in every mix, if it has sound.
	/// A media file without any has none once it opens.
	pub fn strip(&self) -> Option<&Strip> {
		self.strip.as_ref().filter(|_| !self.silent.load(Ordering::Relaxed))
	}

	/// A browser source's page: how it paints, and interaction with it.
	pub fn page(&self) -> Option<&Page> {
		self.page.as_ref()
	}

	/// The newest picture not yet drawn.
	pub(crate) fn take(&self) -> Option<Taken> {
		self.picture.lock().unwrap().take()
	}

	/// The newest picture not yet drawn, without whether it follows a clear.
	#[cfg(test)]
	pub(crate) fn next_picture(&self) -> Option<Picture> {
		self.take().map(|taken| taken.picture)
	}
}

/// The work behind one [`Feed`]: opening the source and delivering what it
/// produces. Built by [`Sources::sync`].
pub struct Driver {
	name: String,
	source: Source,
	context: Context,
	picture: Mailbox,
	delivered: Delivered,
	strip: Option<Strip>,
	status: kio::Producer<Status>,
	#[cfg_attr(
		not(target_os = "linux"),
		expect(dead_code, reason = "browser sources are Linux-only")
	)]
	page: Option<Page>,
	/// The source's own track, when it publishes one.
	track: Option<Track>,
	played: Playback,
	plays: Option<watch::Receiver<Next>>,
	cues: Option<watch::Receiver<Cue>>,
	runs: Runs,
	silent: Arc<AtomicBool>,
	clock: Option<timeline::Producer>,
	/// A layer's broadcast path, and the clock of the broadcast source it
	/// plays on.
	follows: Option<(String, timeline::Consumer)>,
}

/// Where a source's own track goes, its claim on the local catalog's listing
/// under its name, and what the track is doing.
struct Track {
	publish: Publish,
	claim: Claim,
	status: kio::Producer<Status>,
}

impl Driver {
	/// The source's name in the collection.
	pub fn name(&self) -> &str {
		&self.name
	}

	/// Run the source until its [`Feed`] is dropped.
	///
	/// A failure is reported through [`Feed::status`] rather than returned, and
	/// a track's through [`Feed::track`]. On macOS a capture source's future is
	/// `!Send`, so run it on a local task.
	pub async fn run(self) {
		let status = self.status.clone();
		let listing = self.listing();
		let result = tokio::select! {
			_ = status.wanted_changed(true) => return,
			result = self.produce(listing.as_ref()) => result,
			() = self.mirror(listing.as_ref()) => unreachable!("a mirror never ends"),
		};
		let reason = match result {
			Ok(()) => "the source ended".into(),
			Err(err) => err.to_string(),
		};
		self.stop(reason);
	}

	/// Fail the source without running it, such as when its thread cannot
	/// start, so its [`Feed`] says why rather than starting forever.
	pub fn fail(self, err: impl std::fmt::Display) {
		self.stop(err.to_string());
	}

	fn stop(&self, reason: String) {
		let status = Status::Failed(reason);
		tracing::warn!(source = %self.name, ?status, "source stopped");
		self.fail_track(status.clone());
		set(&self.status, status);
	}

	/// The source's own catalog, listed in the studio's, when it publishes.
	fn listing(&self) -> Option<Listing<'_>> {
		let track = self.track.as_ref()?;
		Listing::new(&track.claim, &track.publish)
			.inspect_err(|err| self.fail_track(Status::Failed(err.to_string())))
			.ok()
	}

	/// Keep the local catalog listing what `listing` publishes. A catalog that
	/// closes fails the track, and the source keeps running.
	async fn mirror(&self, listing: Option<&Listing<'_>>) {
		if let Some(listing) = listing
			&& let Err(err) = listing.mirror().await
		{
			self.fail_track(Status::Failed(err.to_string()));
		}
		std::future::pending().await
	}

	fn set_track(&self, value: Status) {
		if let Some(track) = &self.track {
			set(&track.status, value);
		}
	}

	/// Fail the track, unless it already failed with its own reason.
	fn fail_track(&self, value: Status) {
		if let Some(track) = &self.track
			&& !matches!(*track.status.read(), Status::Failed(_))
		{
			set(&track.status, value);
		}
	}

	async fn produce(&self, listing: Option<&Listing<'_>>) -> Result<(), Error> {
		match &self.source {
			Source::Color { color } => {
				self.show(Picture::Rgba {
					size: moq_video::Size::new(1, 1),
					pixels: color.0.to_vec(),
				});
				let _still = listing.and_then(|listing| self.publish(Still::color(listing, *color)));
				std::future::pending().await
			}
			Source::Image { path } => {
				let (picture, bytes) = image(&self.context.dir.join(path))?;
				self.show(picture);
				let _still = listing.and_then(|listing| self.publish(Still::new(listing, bytes)));
				std::future::pending().await
			}
			Source::Lut { path } => {
				let path = self.context.dir.join(path);
				let failed = |err: String| Error::Lut(format!("{}: {err}", path.display()));
				let text = std::fs::read_to_string(&path).map_err(|err| failed(err.to_string()))?;
				let cube = Cube::parse(&text).map_err(failed)?;
				self.show(Picture::Lut(cube));
				std::future::pending().await
			}
			Source::Overlay { html, .. } => {
				let content = crate::overlay::check(html, &self.context.dir)?;
				self.show(Picture::Overlay(content));
				std::future::pending().await
			}
			Source::Media { path, looping, .. } => {
				let cues = self.cues.clone().expect("a media source has cues");
				self.media(&self.context.dir.join(path), *looping, cues, listing).await
			}
			Source::Broadcast { path } => self.broadcast(path).await,
			Source::Layer { source, .. } => self.layer(source).await,
			Source::Browser { url, width, height } => self.browser(url, *width, *height).await,
			Source::Replay => {
				let plays = self.plays.clone().expect("a replay source has plays");
				self.replays(plays, listing).await
			}
			Source::Clone { .. } | Source::Scene { .. } | Source::Program => {
				unreachable!("a clone or scene opens no feed")
			}
			device => self.device(device, listing).await,
		}
	}

	/// Report how publishing went, keeping what was published.
	fn publish<T>(&self, result: Result<T, Error>) -> Option<T> {
		match result {
			Ok(published) => {
				self.set_track(Status::Live);
				Some(published)
			}
			Err(err) => {
				tracing::warn!(source = %self.name, %err, "cannot publish the source");
				self.fail_track(Status::Failed(err.to_string()));
				None
			}
		}
	}

	fn show(&self, picture: Picture) {
		self.picture.lock().unwrap().show(picture);
		*self.delivered.lock().unwrap() = Some(kio::clock::now());
		set(&self.status, Status::Live);
	}

	/// Capture a camera, display, window, or audio input. A picture is also
	/// encoded onto the source's own track as it is shown, when `listing` is set.
	#[cfg(feature = "capture")]
	async fn device(&self, source: &Source, listing: Option<&Listing<'_>>) -> Result<(), Error> {
		use moq_audio::capture::Source as Audio;
		use moq_video::capture::Source as Video;

		let video = match source {
			Source::Camera { id } => Video::Camera(id.clone()),
			Source::Display { id } => Video::Display(id.clone()),
			Source::Window { id } => Video::Window(id.clone()),
			Source::Microphone { id } => return self.listen(Audio::Microphone(id.clone()), listing).await,
			Source::SystemAudio => return self.listen(Audio::System, listing).await,
			Source::Color { .. }
			| Source::Image { .. }
			| Source::Media { .. }
			| Source::Broadcast { .. }
			| Source::Layer { .. }
			| Source::Browser { .. }
			| Source::Overlay { .. }
			| Source::Clone { .. }
			| Source::Lut { .. }
			| Source::Replay
			| Source::Scene { .. }
			| Source::Program => unreachable!("not a device source"),
		};

		let mut config = moq_video::capture::Config::default();
		config.source = video;
		config.output = self.context.output;
		let mut stream = moq_video::capture::open(&config).await?;

		let mut track = match listing {
			Some(listing) => {
				let size = moq_video::Size::new(stream.width(), stream.height());
				let framerate = stream.framerate().unwrap_or_else(default_framerate);
				let video = crate::publish::Video::new(listing, size, framerate, stream.color()).await;
				self.publish(video)
			}
			None => None,
		};

		// A display or window delivers nothing while its screen stands still,
		// so going into use reopens its capture, which starts with the picture
		// as it is. A camera delivers its next picture within a frame anyway.
		let still = matches!(source, Source::Display { .. } | Source::Window { .. });
		let result = async {
			// Whether a change of use is still to be acted on: not again until
			// a picture arrives, since a closed encoder opens only on one.
			let mut settling = true;
			loop {
				let settled = async {
					match &track {
						Some(video) if settling => video.settle().await,
						_ => std::future::pending().await,
					}
				};
				// Leaving use gives the encode session back at once, and going
				// into use brings a picture, even while the capture delivers
				// nothing new.
				let frame = tokio::select! {
					frame = stream.read() => frame?,
					() = settled => {
						let Some(video) = &mut track else { continue };
						if video.is_open() {
							if let Err(err) = video.release() {
								self.fail_encoding(&mut track, err);
							}
						} else {
							settling = false;
							if still {
								// Closed before the next opens, so the device is
								// captured once.
								drop(stream);
								stream = moq_video::capture::open(&config).await?;
								video.reopened();
							}
						}
						continue;
					}
				};
				settling = true;
				let Some(mut frame) = frame else { break };
				if let Some(video) = &mut track
					&& let Err(err) = video.encode(&mut frame)
				{
					self.fail_encoding(&mut track, err);
				}
				self.show(Picture::Frame(frame));
			}
			Ok(())
		}
		.await;
		if let (Some(video), Err(err)) = (track, &result) {
			video.abort(err);
		}
		result
	}

	/// Fail the source's own track with `err`, ending it, while the source
	/// keeps drawing.
	fn fail_encoding(&self, track: &mut Option<crate::publish::Video<'_>>, err: Error) {
		tracing::warn!(source = %self.name, %err, "cannot encode the source");
		self.fail_track(Status::Failed(err.to_string()));
		if let Some(video) = track.take() {
			video.abort(&err);
		}
	}

	/// Capture a microphone or system audio into the bus, and with `listing`,
	/// onto the source's own track as well, from the one device open.
	#[cfg(feature = "capture")]
	async fn listen(&self, source: moq_audio::capture::Source, listing: Option<&Listing<'_>>) -> Result<(), Error> {
		let strip = self.strip.clone().expect("audio sources have a strip");
		let capture = capture(source).await?;
		let channel = self.context.bus.capture(capture.consume(), strip)?;
		let mut track = listing.and_then(|listing| {
			let input = capture.consume().input().clone();
			self.publish(crate::publish::Audio::new(listing, input))
		});
		let heard = self.hear(capture.consume(), &mut track);
		set(&self.status, Status::Live);
		let result = tokio::try_join!(
			async { capture.run().await.map_err(Error::from) },
			async { channel.run().await.map_err(Error::from) },
			heard
		);
		if let (Some(audio), Err(err)) = (track, &result) {
			audio.abort(err);
		}
		result.map(|_| ())
	}

	/// Note each buffer `capture` delivers, so one that goes quiet reads as
	/// stalled, and encode it onto `track` while the source is used. Ends with
	/// the capture's error.
	#[cfg(feature = "capture")]
	async fn hear(
		&self,
		mut capture: moq_audio::capture::Consumer,
		track: &mut Option<crate::publish::Audio<'_>>,
	) -> Result<(), Error> {
		loop {
			let buffer = capture.read().await?;
			*self.delivered.lock().unwrap() = Some(kio::clock::now());
			if let Some(audio) = track
				&& let Err(err) = audio.write(&buffer.data, buffer.captured, buffer.gap)
			{
				tracing::warn!(source = %self.name, %err, "cannot encode the source");
				self.fail_track(Status::Failed(err.to_string()));
				track.take().expect("encoding").abort(&err);
			}
		}
	}

	#[cfg(not(feature = "capture"))]
	async fn device(&self, _: &Source, _: Option<&Listing<'_>>) -> Result<(), Error> {
		Err(Error::Unsupported(
			"this build cannot capture devices (enable the `capture` feature)".into(),
		))
	}

	/// Render the page at `url` until its helper stops, or fail at once if the
	/// studio offers no helper.
	#[cfg(target_os = "linux")]
	async fn browser(&self, url: &str, width: u32, height: u32) -> Result<(), Error> {
		let Some(helper) = &self.context.browser else {
			return Err(Error::Unsupported(
				"this build cannot render web pages (enable the `cef` feature)".into(),
			));
		};
		let page = self.page.as_ref().expect("browser sources have a page");
		let bus = self.context.bus.config().clone();
		let open = crate::browser::Open {
			url: url.to_string(),
			width,
			height,
			sample_rate: bus.sample_rate,
			channels: bus.layout.channels(),
			..Default::default()
		};
		let mut input = moq_audio::encode::Input::new(bus.sample_rate, bus.layout);
		input.format = moq_audio::Format::F32;
		let strip = self.strip.clone().expect("browser sources have a strip");
		let sound = self.context.bus.live(input, strip)?;
		crate::browser::run(helper, open, page, &self.picture, Some(sound), &self.status).await
	}

	#[cfg(not(target_os = "linux"))]
	async fn browser(&self, _: &str, _: u32, _: u32) -> Result<(), Error> {
		Err(Error::Unsupported("browser sources run on Linux only for now".into()))
	}

	/// Follow the broadcast at `path` for as long as the feed lives, playing
	/// each instance of it that comes online.
	async fn broadcast(&self, path: &str) -> Result<(), Error> {
		let mut clock = self.clock.clone().expect("a broadcast source has a clock");
		self.follow(path, |broadcast| {
			*self.played.lock().unwrap() = Played {
				broadcast: Some(broadcast.clone()),
				..Played::default()
			};
			// Each instance's timestamps are its own.
			clock = clock.clone().reset(VIDEO_DELAY);
			Box::pin(self.play(broadcast, path, clock.clone()))
		})
		.await
	}

	/// Play the source called `source` that the broadcast this layer follows
	/// publishes independently, from each instance of it, on the clock of the
	/// broadcast source playing it.
	async fn layer(&self, source: &str) -> Result<(), Error> {
		let Some((path, schedule)) = &self.follows else {
			return Err(Error::Invalid(format!(
				"layer `{}` follows no broadcast source",
				self.name
			)));
		};
		self.follow(path, |broadcast| {
			Box::pin(self.play_layer(broadcast, path, source, schedule))
		})
		.await
	}

	/// Play each instance of the broadcast at `path` with `start` as it comes
	/// online, until the next replaces it or it goes offline.
	async fn follow<'a>(
		&'a self,
		path: &str,
		mut start: impl FnMut(moq_net::broadcast::Consumer) -> Playing<'a>,
	) -> Result<(), Error> {
		let mut instances = self.context.origin.follow(path, None)?;
		set(&self.status, Status::Offline);

		let mut playing: Option<Playing<'_>> = None;
		loop {
			tokio::select! {
				event = instances.available() => match event {
					Ok(Some(broadcast)) => {
						set(&self.status, Status::Live);
						playing = Some(start(broadcast));
					}
					Ok(None) => {
						playing = None;
						*self.played.lock().unwrap() = Played::default();
						set(&self.status, Status::Offline);
					}
					Err(err) => return Err(err.into()),
				},
				result = async { playing.as_mut().expect("guarded").await }, if playing.is_some() => {
					playing = None;
					set(&self.status, match result {
						Ok(()) => Status::Offline,
						Err(err) => Status::Failed(err.to_string()),
					});
				}
			}
		}
	}

	/// Decode one instance of a studio broadcast's source called `source`:
	/// the first video and audio renditions its entry under the catalog's
	/// [`SECTION`](crate::publish::SECTION) `sources` offers, switching as
	/// the catalog changes them, until the catalog ends. Both play on
	/// `schedule`, the program's clock, without moving it.
	async fn play_layer(
		&self,
		broadcast: moq_net::broadcast::Consumer,
		path: &str,
		source: &str,
		schedule: &timeline::Consumer,
	) -> Result<(), Error> {
		let format = moq_mux::catalog::CatalogFormat::detect(path).unwrap_or_default();
		let mut catalogs = moq_mux::catalog::Consumer::<crate::publish::Ext>::new(&broadcast, format).await?;
		let mut video: Option<String> = None;
		let mut audio: Option<String> = None;
		let mut video_playing: Playing<'_> = Box::pin(std::future::pending());
		let mut audio_playing: Playing<'_> = Box::pin(std::future::pending());
		loop {
			tokio::select! {
				catalog = catalogs.next() => {
					let Some(catalog) = catalog? else {
						return Ok(());
					};
					let listed = catalog.ext.studio.sources.get(source);
					let next_video = listed.map(|listed| pick(listed.video.enabled())).transpose()?.flatten();
					let next_audio = listed.map(|listed| pick(listed.audio.enabled())).transpose()?.flatten();
					// A source without video, such as an image or a color, is a still.
					let still = listed.and_then(|listed| {
						let mut stills = listed.binary.tracks.iter();
						let (id, config) = stills.find(|(_, config)| config.broadcast.is_none() && config.mime.as_deref().is_some_and(|mime| mime.starts_with("image/")))?;
						Some(Reference::track(config, id).to_string())
					});
					let track = match &next_video {
						Some((id, config)) => Some(Reference::track(config, id).to_string()),
						None => still.clone(),
					};
					if track != video {
						video = track;
						video_playing = match (next_video, still) {
							(Some((id, config)), _) => Box::pin(self.follow_video(&broadcast, id, config, schedule)),
							(None, Some(still)) => Box::pin(self.follow_still(&broadcast, still)),
							(None, None) => {
								self.picture.lock().unwrap().clear();
								Box::pin(std::future::pending())
							}
						};
					}
					let track = next_audio.as_ref().map(|(id, config)| Reference::track(config, id).to_string());
					if track != audio {
						audio = track;
						audio_playing = match next_audio {
							Some((id, config)) => Box::pin(self.follow_audio(&broadcast, id, config, schedule)),
							None => Box::pin(std::future::pending()),
						};
					}
				}
				result = &mut video_playing => {
					result?;
					video_playing = Box::pin(std::future::pending());
				}
				result = &mut audio_playing => {
					result?;
					audio_playing = Box::pin(std::future::pending());
				}
			}
		}
	}

	/// Show the newest picture of the track `name`, a PNG or JPEG file per
	/// group, as [`Still`] publishes one, whenever it arrives: a still has no
	/// moment.
	async fn follow_still(&self, broadcast: &moq_net::broadcast::Consumer, name: String) -> Result<(), Error> {
		let mut track = broadcast.track(&name)?.subscribe(None).await?;
		while let Some(mut group) = track.recv_group().await? {
			if let Some(frame) = group.read_frame().await? {
				self.show(decode(&frame.payload).map_err(|err| Error::Image(format!("{name}: {err}")))?);
			}
		}
		Ok(())
	}

	/// Show each picture once `schedule`, another track's clock, says it is
	/// due and its sound stamped alike has played, the newest such one when
	/// several are, as [`play_video`](Self::play_video) does.
	async fn follow_video(
		&self,
		broadcast: &moq_net::broadcast::Consumer,
		name: String,
		config: hang::catalog::VideoConfig,
		schedule: &timeline::Consumer,
	) -> Result<(), Error> {
		let mut options = moq_video::decode::Options::new();
		options.start = moq_video::decode::Start::Latest;
		options.decoder.output = self.context.output;
		options.delay = moq_video::decode::Delay {
			min: Some(VIDEO_DELAY),
			max: Some(MAX_DELAY),
		};
		let mut consumer = moq_video::decode::Consumer::new(broadcast, &config, name, options).await?;
		let mut queue = VecDeque::<moq_video::Frame>::new();
		let mut ended = false;
		loop {
			let now = kio::clock::now();
			// Counted first: `present` holds the clock's lock while it iterates.
			let heard_count = queue
				.iter()
				.take_while(|frame| heard(schedule, frame.timestamp, now))
				.count();
			let ready = queue.iter().take(heard_count).map(|frame| frame.timestamp);
			if let Some(index) = schedule.present(ready, now) {
				let frame = queue.drain(..=index).next_back().expect("an index into the queue");
				self.show(Picture::Frame(frame));
			}
			if ended && queue.is_empty() {
				return Ok(());
			}
			// Its moment, or once due, soon to see whether its sound has played.
			let wake = queue.front().and_then(|frame| match schedule.due(frame.timestamp) {
				Some(due) if due > now => Some(due),
				_ if !heard(schedule, frame.timestamp, now) => Some(now + crate::playout::RECHECK),
				_ => None,
			});
			tokio::select! {
				read = consumer.read(), if !ended && queue.len() < QUEUED => match read? {
					Some(moq_video::decode::Read::Frame(frame)) => queue.push_back(frame),
					Some(moq_video::decode::Read::Gap { .. } | moq_video::decode::Read::End { .. }) => {}
					None => ended = true,
				},
				() = kio::clock::sleep_until(wake.unwrap_or(now)), if wake.is_some() => {}
			}
		}
	}

	/// Play a track's sound into the bus when `schedule`, another track's
	/// clock, says each frame sounds and its own sound stamped alike has been
	/// written, dropping a frame too late for its moment, or before the clock
	/// has one.
	async fn follow_audio(
		&self,
		broadcast: &moq_net::broadcast::Consumer,
		name: String,
		config: hang::catalog::AudioConfig,
		schedule: &timeline::Consumer,
	) -> Result<(), Error> {
		let Some(strip) = self.strip.clone() else {
			return Ok(());
		};
		let bus = self.context.bus.config();
		let mut options = moq_audio::decode::Options::new();
		options.start = moq_audio::decode::Start::Latest;
		options.delay = moq_audio::decode::Delay {
			min: None,
			max: Some(MAX_DELAY),
		};
		options.output.format = moq_audio::Format::F32;
		options.output.sample_rate = Some(bus.sample_rate);
		options.output.layout = Some(bus.layout);
		let mut consumer = moq_audio::decode::Consumer::new(broadcast, &config, name, options).await?;
		let mut writer = self.writer(strip)?;

		// Each frame waits here until its lead before it sounds.
		let mut queue = VecDeque::<moq_audio::Frame>::new();
		let mut ended = false;
		loop {
			let now = kio::clock::now();
			let mut write = None;
			while let Some(frame) = queue.front() {
				match follow(schedule, frame.timestamp, now) {
					Follow::Write(sounds) => {
						let frame = queue.pop_front().expect("peeked");
						writer.write(sounds, &frame.data).await?;
					}
					Follow::Wait(at) => {
						write = Some(at);
						break;
					}
					Follow::Drop => {
						queue.pop_front();
					}
				}
			}
			if ended && queue.is_empty() {
				return Ok(());
			}
			tokio::select! {
				read = consumer.read(), if !ended => match read? {
					Some(moq_audio::decode::Read::Frame(frame)) => {
						// One due past the longest target would hold whatever a
						// peer stamps.
						let now = kio::clock::now();
						let placed = schedule.due(frame.timestamp).is_some_and(|sounds| {
							sounds >= now && sounds <= now + MAX_DELAY + WRITE_LEAD
						});
						if placed {
							queue.push_back(frame);
						}
					}
					// A hole or an encoder restart shows in the stamps that follow.
					Some(_) => {}
					None => ended = true,
				},
				() = kio::clock::sleep_until(write.unwrap_or(now)), if write.is_some() => {}
			}
		}
	}

	/// Decode one instance of a broadcast: the first video and audio renditions
	/// each catalog offers, switching when a catalog update changes them, until
	/// the catalog ends. Both play on `clock`.
	async fn play(
		&self,
		broadcast: moq_net::broadcast::Consumer,
		path: &str,
		clock: timeline::Producer,
	) -> Result<(), Error> {
		let format = moq_mux::catalog::CatalogFormat::detect(path).unwrap_or_default();
		let mut catalogs = moq_mux::catalog::Consumer::<()>::new(&broadcast, format).await?;
		let clock = Clock {
			timeline: clock,
			changed: Notify::new(),
		};

		// A track name always means the same content, so only a new track
		// restarts a decoder. That is the track an entry resolves to, not its
		// catalog ID: a publisher may keep the ID and point it at a replacement
		// track, as the JS publisher does on a codec change. A metadata-only
		// update, such as a jitter estimate, leaves playback alone. Video also
		// restarts when audio comes or goes, which changes how long it waits.
		let mut video: Option<(String, bool)> = None;
		let mut audio: Option<String> = None;
		let mut video_playing: Playing<'_> = Box::pin(std::future::pending());
		let mut audio_playing: Playing<'_> = Box::pin(std::future::pending());

		loop {
			tokio::select! {
				catalog = catalogs.next() => {
					let Some(catalog) = catalog? else {
						return Ok(());
					};
					let next_video = pick(catalog.video.enabled())?;
					let next_audio = pick(catalog.audio.enabled())?;
					let track = next_video
						.as_ref()
						.map(|(id, config)| (Reference::track(config, id).to_string(), next_audio.is_some()));
					if track != video {
						video = track;
						let mut played = self.played.lock().unwrap();
						played.video = next_video.clone();
						// The newest picture was the old track's, a moment the
						// new one may not hold.
						played.newest = None;
						drop(played);
						// Video waits for its audio as long as audio's target can
						// grow; alone, it waits only its own delay.
						let max = if next_audio.is_some() { MAX_DELAY } else { VIDEO_DELAY };
						video_playing = match next_video {
							Some((id, config)) => Box::pin(self.play_video(&broadcast, id, config, &clock, max)),
							None => Box::pin(std::future::pending()),
						};
					}
					let track = next_audio.as_ref().map(|(id, config)| Reference::track(config, id).to_string());
					if track != audio {
						audio = track;
						self.played.lock().unwrap().audio = next_audio.clone();
						audio_playing = match next_audio {
							Some((id, config)) => Box::pin(self.play_audio(&broadcast, id, config, &clock)),
							None => Box::pin(std::future::pending()),
						};
					}
				}
				// A rendition that ends waits for a catalog update to replace it.
				result = &mut video_playing => {
					result?;
					video_playing = Box::pin(std::future::pending());
				}
				result = &mut audio_playing => {
					result?;
					audio_playing = Box::pin(std::future::pending());
				}
			}
		}
	}

	/// Show each picture once `clock` says it is due, the newest due one when
	/// several are, holding a few decoded ones for their moments. The canvas
	/// keeps showing the last picture meanwhile.
	async fn play_video(
		&self,
		broadcast: &moq_net::broadcast::Consumer,
		name: String,
		config: hang::catalog::VideoConfig,
		clock: &Clock,
		max: Duration,
	) -> Result<(), Error> {
		// A replacement track's stamps need not continue the old one's.
		clock.timeline.video_restarted();
		let mut options = moq_video::decode::Options::new();
		options.start = moq_video::decode::Start::Latest;
		options.decoder.output = self.context.output;
		options.delay = moq_video::decode::Delay {
			min: Some(VIDEO_DELAY),
			max: Some(max),
		};
		let mut consumer = moq_video::decode::Consumer::new(broadcast, &config, name, options).await?;
		let schedule = clock.timeline.consume();
		let mut queue = VecDeque::<moq_video::Frame>::new();
		let mut ended = false;
		loop {
			let now = kio::clock::now();
			// Counted first: `present` holds the clock's lock while it iterates.
			let heard = queue
				.iter()
				.take_while(|frame| crate::playout::heard(&schedule, frame.timestamp, now))
				.count();
			let ready = queue.iter().take(heard).map(|frame| frame.timestamp);
			if let Some(index) = schedule.present(ready, now) {
				let frame = queue.drain(..=index).next_back().expect("an index into the queue");
				self.played.lock().unwrap().newest = Some(frame.timestamp);
				self.show(Picture::Frame(frame));
			}
			if ended && queue.is_empty() {
				return Ok(());
			}
			// A picture due but not yet heard waits for audio to place more.
			let due = queue
				.front()
				.and_then(|frame| schedule.due(frame.timestamp))
				.filter(|due| *due > now);
			tokio::select! {
				read = consumer.read(), if !ended && queue.len() < QUEUED => match read? {
					Some(moq_video::decode::Read::Frame(frame)) => {
						clock.timeline.video(frame.timestamp, kio::clock::now());
						queue.push_back(frame);
					}
					Some(moq_video::decode::Read::Gap { .. } | moq_video::decode::Read::End { .. }) => {
						clock.timeline.video_restarted();
					}
					None => ended = true,
				},
				() = kio::clock::sleep_until(due.unwrap_or(now)), if due.is_some() => {}
				() = clock.changed.notified() => {}
			}
		}
	}

	/// Play the broadcast's sound into the bus on its timestamps, holding the
	/// jitter estimate moq-audio's decoder makes, and anchor `clock` on it.
	async fn play_audio(
		&self,
		broadcast: &moq_net::broadcast::Consumer,
		name: String,
		config: hang::catalog::AudioConfig,
		clock: &Clock,
	) -> Result<(), Error> {
		let Some(strip) = self.strip.clone() else {
			return Ok(());
		};
		let bus = self.context.bus.config();
		let mut options = moq_audio::decode::Options::new();
		options.start = moq_audio::decode::Start::Latest;
		options.delay = moq_audio::decode::Delay {
			min: None,
			max: Some(MAX_DELAY),
		};
		options.output.format = moq_audio::Format::F32;
		options.output.sample_rate = Some(bus.sample_rate);
		options.output.layout = Some(bus.layout);
		let mut consumer = moq_audio::decode::Consumer::new(broadcast, &config, name, options).await?;

		let mut writer = self.writer(strip)?;
		let speaker = RefCell::new(Speaker::new(clock.timeline.clone(), &clock.changed));
		let stride = bus.layout.channels() as usize * size_of::<f32>();
		let rate = f64::from(bus.sample_rate);

		// Frames wait here for their moment while every arrival is read at
		// once: the jitter estimate times each one as `read` hands it over. The
		// speaker places none past its target, and a frame that has finished
		// sounding by the time another arrives is dropped as too late, so
		// however long a write waits on the bus, this holds only what is
		// still to sound within the longest target.
		let queue = RefCell::new(VecDeque::<(Instant, Instant, moq_audio::Frame)>::new());
		let queued = Notify::new();
		let received = Cell::new(false);
		let receive = async {
			while let Some(next) = consumer.read().await? {
				let moq_audio::decode::Read::Frame(frame) = next else {
					// A hole or an encoder restart shows in the stamps that follow.
					continue;
				};
				let duration = Duration::from_secs_f64((frame.data.len() / stride) as f64 / rate);
				let now = kio::clock::now();
				let place = speaker
					.borrow_mut()
					.place(frame.timestamp.into(), duration, consumer.delay(), now);
				if let Place::Play(sounds) = place {
					let mut queue = queue.borrow_mut();
					while queue.front().is_some_and(|(_, ends, _)| *ends <= now) {
						queue.pop_front();
					}
					queue.push_back((sounds, sounds + duration, frame));
					queued.notify_one();
				}
			}
			received.set(true);
			queued.notify_one();
			Ok::<_, Error>(())
		};
		// Writes each frame its lead ahead, and tells the clock about
		// the frames after a skip once they sound.
		let write = async {
			loop {
				let now = kio::clock::now();
				speaker.borrow_mut().commit(now);
				let write = queue.borrow().front().map(|(sounds, ..)| *sounds - WRITE_LEAD);
				if write.is_some_and(|at| at <= now) {
					let (sounds, _, frame) = queue.borrow_mut().pop_front().expect("peeked");
					writer.write(sounds, &frame.data).await?;
					continue;
				}
				let wake = write.into_iter().chain(speaker.borrow().deferred()).min();
				match wake {
					Some(at) => tokio::select! {
						() = kio::clock::sleep_until(at) => {}
						() = queued.notified() => {}
					},
					None if received.get() => return Ok::<_, Error>(()),
					None => queued.notified().await,
				}
			}
		};
		tokio::try_join!(receive, write)?;
		Ok(())
	}

	/// Play each replay [`Feed::replay`] hands over, showing nothing between
	/// them. A new one cuts off the one playing.
	async fn replays(&self, mut plays: watch::Receiver<Next>, listing: Option<&Listing<'_>>) -> Result<(), Error> {
		self.picture.lock().unwrap().clear();
		set(&self.status, Status::Live);

		let mut playing: Option<Playing<'_>> = None;
		loop {
			tokio::select! {
				changed = plays.changed() => {
					// The feed is gone, which stops this driver anyway.
					if changed.is_err() {
						return std::future::pending().await;
					}
					let play = plays.borrow_and_update().lock().unwrap().take();
					playing =play.map(|play| Box::pin(self.replay(play, listing)) as Playing<'_>);
				}
				result = async { playing.as_mut().expect("guarded").await }, if playing.is_some() => {
					playing = None;
					self.picture.lock().unwrap().clear();
					set(&self.status, match result {
						Ok(()) => Status::Live,
						Err(err) => Status::Failed(err.to_string()),
					});
				}
			}
		}
	}

	/// Play one take at its speed, each picture and sound at its moment: one
	/// `t` into the span plays `t / speed` after the replay begins, once the
	/// first of each is decoded and the bus can sound it. Ends once the whole
	/// span has played.
	///
	/// Each sound is handed to the bus [`WRITE_LEAD`] ahead of its moment.
	///
	/// With `listing`, the pictures are also encoded onto the source's own
	/// track as they show, while it is used.
	async fn replay(&self, play: Play, listing: Option<&Listing<'_>>) -> Result<(), Error> {
		set(&self.status, Status::Live);
		let take = &play.take;
		let broadcast = take.consume();
		let mut video = match &take.video {
			Some((name, config)) => {
				let consumer = moq_video::decode::Consumer::new(&broadcast, config, name, self.pictures()).await?;
				Some((consumer, config))
			}
			None => None,
		};
		// Slowed down, sound would need stretching, so it plays only in real time.
		let mut audio = match (&take.audio, &self.strip) {
			(Some((name, config)), Some(strip)) if play.speed == 1.0 => {
				let consumer =
					moq_audio::decode::Consumer::new(&broadcast, config, name.clone(), self.sounds()).await?;
				Some((consumer, self.writer(strip.clone())?))
			}
			_ => None,
		};

		// The first of each from the start on is decoded before the clock
		// starts, so neither is late. The group in progress at the start
		// decodes from its keyframe, but plays only from the start on. A gap
		// or end before the first picture leaves nothing to clear or mark.
		let mut picture = None;
		if let Some((consumer, _)) = &mut video {
			while let Some(read) = consumer.read().await? {
				if let moq_video::decode::Read::Frame(frame) = &read
					&& frame.timestamp >= take.start
				{
					picture = Some(read);
					break;
				}
			}
		}
		// A sound across the start plays from where the replay begins.
		let mut samples = None;
		if let Some((consumer, _)) = &mut audio {
			let bus = self.context.bus.config();
			let stride = bus.layout.channels() as usize * size_of::<f32>();
			while let Some(frame) = next_samples(consumer).await? {
				let length = (frame.data.len() / stride) as f64 / f64::from(bus.sample_rate);
				if Duration::from(frame.timestamp) + Duration::from_secs_f64(length) > Duration::from(take.start) {
					samples = Some(frame);
					break;
				}
			}
		}
		// The pictures' track opens before the clock starts too, so the
		// encoder's probe holds back neither the first picture nor the sound
		// beside it.
		let mut own = Own::new(listing, kio::clock::now());
		if let (Some(moq_video::decode::Read::Frame(frame)), Some((_, config))) = (&picture, &video) {
			self.open(&mut own, frame, config).await;
		}
		let begin = kio::clock::now() + WRITE_LEAD;
		self.ran(play.play, |run| {
			run.begun.get_or_insert(begin);
		});
		let at = |timestamp: moq_net::Timestamp| {
			let (timestamp, start) = (Duration::from(timestamp), Duration::from(take.start));
			match timestamp.checked_sub(start) {
				Some(into) => begin + into.div_f32(play.speed),
				None => begin - (start - timestamp).div_f32(play.speed),
			}
		};

		let pictures = async {
			let Some((consumer, config)) = &mut video else {
				return Ok(());
			};
			while let Some(read) = picture.take() {
				match read {
					moq_video::decode::Read::Frame(frame) if frame.timestamp < take.start => {}
					// One at the end would show for no time at all.
					moq_video::decode::Read::Frame(frame) if frame.timestamp >= take.end => break,
					moq_video::decode::Read::Frame(mut frame) => {
						kio::clock::sleep_until(at(frame.timestamp)).await;
						self.encode(&mut own, &mut frame, config, kio::clock::now()).await?;
						self.show_play(play.play, Picture::Frame(frame));
					}
					// A stretch the broadcast was offline, paused, or lost
					// replays as the gap it was, on its own track too. Past
					// the end, the replay is over anyway.
					moq_video::decode::Read::Gap { timestamp, .. }
					| moq_video::decode::Read::End { end: timestamp } => {
						kio::clock::sleep_until(at(timestamp.clamp(take.start, take.end))).await;
						self.picture.lock().unwrap().clear();
						if let Some(video) = &mut own.track
							&& let Err(err) = video.discontinuity()
						{
							self.fail_encoding(&mut own.track, err);
						}
					}
				}
				picture = consumer.read().await?;
			}
			Ok::<_, Error>(())
		};
		let sounds = async {
			let Some((consumer, writer)) = &mut audio else {
				return Ok(());
			};
			let bus = self.context.bus.config();
			let stride = bus.layout.channels() as usize * size_of::<f32>();
			while let Some(frame) = samples.take()
				&& frame.timestamp < take.end
			{
				let due = at(frame.timestamp);
				ahead(due).await;
				let before = begin.saturating_duration_since(due).as_nanos() * u128::from(bus.sample_rate);
				let cut = (before / 1_000_000_000) as usize * stride;
				writer
					.write(due.max(begin), frame.data.get(cut..).unwrap_or_default())
					.await?;
				samples = next_samples(consumer).await?;
			}
			// The bus drops what a closed channel has not played, so it stays
			// open until the replay ends, by when the last sound has played.
			std::future::pending().await
		};

		// The span ends where it does even when its tracks have not ended yet.
		let played = async {
			tokio::try_join!(pictures, sounds)?;
			std::future::pending().await
		};
		let end = at(take.end);
		// Played first, so a picture due at the very end shows before it does.
		tokio::select! {
			biased;
			result = played => return result,
			() = kio::clock::sleep_until(end) => {}
		}
		self.ran(play.play, |run| {
			run.ended.get_or_insert(end);
		});
		Ok(())
	}

	/// Play the media file at `path` as `cues` asks: from its start for each
	/// play, cutting off the play under way, and blank and silent while
	/// stopped. At its end it starts over when `looping`, or holds its last
	/// picture, silent, until the next.
	///
	/// With `listing`, the pictures are also encoded onto the source's own
	/// track, stamped when they show, while it is used, on one timeline across
	/// plays.
	async fn media(
		&self,
		path: &Path,
		looping: bool,
		mut cues: watch::Receiver<Cue>,
		listing: Option<&Listing<'_>>,
	) -> Result<(), Error> {
		let clip = Clip::open(path)?;
		// Without sound it has no strip to mix or meter.
		self.silent.store(clip.audio.is_none(), Ordering::Relaxed);
		let mut sound = match (&clip.audio, &self.strip) {
			(Some(_), Some(_)) => {
				// Its track opens now, the pictures' at the first picture.
				let track = listing.and_then(|listing| {
					let bus = self.context.bus.config();
					let mut input = moq_audio::encode::Input::new(bus.sample_rate, bus.layout);
					input.format = moq_audio::Format::F32;
					self.publish(crate::publish::Audio::new(listing, input))
				});
				Some(Sound {
					writer: None,
					track,
					ended: None,
				})
			}
			_ => None,
		};
		let mut own = Own::new(listing, kio::clock::now());
		let shows = Shows::default();
		// The last channel on a strip to close drops what it has not played
		// yet, so a loop keeps its channel, sounding on, while a new cue drops
		// it, cutting off what the play before still had queued, pictures
		// included; the next play takes a new one. Its pictures start a group
		// there too, a cut to the clip's start.
		let cut = |sound: &mut Option<Sound<'_>>, own: &mut Own<'_, '_>| {
			if let Some(sound) = sound {
				sound.writer = None;
				sound.ended = None;
			}
			shows.queue.borrow_mut().clear();
			if let Some(video) = &mut own.track
				&& let Err(err) = video.cut()
			{
				self.fail_encoding(&mut own.track, err);
			}
		};
		let plays = async {
			let mut cue = *cues.borrow_and_update();
			// Where a loop's next play starts: where the last one ended, so
			// loops keep exact time whatever opening the decoders takes.
			let mut next = None;
			loop {
				// Whether to rest until the next cue, rather than take it at once.
				let rest = if cue.stopped {
					true
				} else {
					if let (Some(sound), Some(strip)) = (&mut sound, &self.strip)
						&& sound.writer.is_none()
					{
						sound.writer = Some(self.writer(strip.clone())?);
					}
					let play = async {
						let end = self
							.play_clip(&clip, sound.as_mut(), &mut own, &shows, next, cue.play)
							.await?;
						// A loop's next play hands its first over now, its
						// lead ahead, as this one did.
						if !looping {
							kio::clock::sleep_until(end).await;
						}
						Ok::<_, Error>(end)
					};
					let played = tokio::select! {
						result = play => Some(result?),
						changed = cues.changed() => {
							// The feed is gone, which stops this driver anyway.
							if changed.is_err() {
								return std::future::pending().await;
							}
							None
						}
					};
					if let Some(end) = played {
						self.ran(cue.play, |run| {
							run.ended.get_or_insert(end);
						});
						next = Some(end);
						if looping {
							continue;
						}
					}
					played.is_some()
				};
				if rest {
					set(&self.status, Status::Ended);
					self.ended(&mut cues, &mut own).await;
				}
				cut(&mut sound, &mut own);
				cue = *cues.borrow_and_update();
				next = None;
			}
		};
		// Shows each picture handed over at its moment, across plays, so a
		// loop's next play can hand its first over before this one's last
		// has shown.
		let present = async {
			loop {
				let now = kio::clock::now();
				let due = shows.queue.borrow().front().map(|(due, ..)| *due);
				match due {
					Some(due) if due <= now => {
						let (_, play, frame) = shows.queue.borrow_mut().pop_front().expect("peeked");
						self.show_play(play, Picture::Frame(frame));
					}
					Some(due) => tokio::select! {
						() = kio::clock::sleep_until(due) => {}
						() = shows.handed.notified() => {}
					},
					None => shows.handed.notified().await,
				}
			}
		};
		tokio::select! {
			result = plays => result,
			() = present => unreachable!("pictures show for as long as the source plays"),
		}
	}

	/// Note how a media or replay source's `play` runs, unless the feed has
	/// asked for another since or stopped it.
	fn ran(&self, play: u64, note: impl FnOnce(&mut Run)) {
		let mut latest = self.runs.lock().unwrap();
		if latest.run.play == play && latest.run.ended.is_none() {
			note(&mut latest.run);
		}
	}

	/// Show a picture of a media or replay source's `play`, unless the feed
	/// has asked for another or stopped it since.
	fn show_play(&self, play: u64, picture: Picture) {
		let latest = self.runs.lock().unwrap();
		if latest.run.play == play && !latest.stopped {
			self.show(picture);
		}
	}

	/// Wait out a media source that ended or stopped until `cues` changes. Its
	/// own track gets no more pictures, so its encoder is released here once
	/// nothing uses the source, giving the encode session back.
	async fn ended(&self, cues: &mut watch::Receiver<Cue>, own: &mut Own<'_, '_>) {
		loop {
			let settled = async {
				match &own.track {
					Some(video) if video.is_open() => video.settle().await,
					_ => std::future::pending().await,
				}
			};
			tokio::select! {
				changed = cues.changed() => {
					// The feed is gone, which stops this driver anyway.
					if changed.is_err() {
						std::future::pending::<()>().await;
					}
					return;
				}
				() = settled => {
					if let Some(video) = &mut own.track
						&& let Err(err) = video.release()
					{
						self.fail_encoding(&mut own.track, err);
					}
				}
			}
		}
	}

	/// Play `clip` once, each picture and sound at its moment, the first at
	/// `begin`, or as soon as the bus can sound it, as part of the feed's
	/// `play`.
	///
	/// Each sound is handed to the bus [`WRITE_LEAD`] ahead of its moment, and
	/// each picture to `shows` then too, to show at the same moment.
	///
	/// Returns where it ends, once all is handed over and the lead before the
	/// end has come, so a loop's next play hands its own first over
	/// in time. A sound lasts for its samples, and the last picture as long as
	/// the one before it. A clip that decodes to nothing fails.
	async fn play_clip(
		&self,
		clip: &Clip,
		mut sound: Option<&mut Sound<'_>>,
		own: &mut Own<'_, '_>,
		shows: &Shows,
		begin: Option<Instant>,
		play: u64,
	) -> Result<Instant, Error> {
		set(&self.status, Status::Live);
		let broadcast = clip.consume();
		let mut video = match &clip.video {
			Some((name, config)) => {
				let consumer =
					moq_video::decode::Consumer::new(&broadcast, config, name.clone(), self.pictures()).await?;
				Some((consumer, config))
			}
			None => None,
		};
		let mut audio = match (&clip.audio, &sound) {
			(Some((name, config)), Some(_)) => {
				Some(moq_audio::decode::Consumer::new(&broadcast, config, name.clone(), self.sounds()).await?)
			}
			_ => None,
		};

		// The first of each is decoded before the clock starts, so the earlier
		// of them is due at once.
		let mut picture = match &mut video {
			Some((consumer, _)) => next_picture(consumer).await?,
			None => None,
		};
		let mut samples = match &mut audio {
			Some(consumer) => next_samples(consumer).await?,
			None => None,
		};
		let first = [
			picture.as_ref().map(|frame| Duration::from(frame.timestamp)),
			samples.as_ref().map(|frame| Duration::from(frame.timestamp)),
		];
		let Some(start) = first.into_iter().flatten().min() else {
			// Its tracks hold no frame that decodes, which a loop would only
			// play again at once, forever.
			return Err(Error::Media("the file decodes to no picture or sound".into()));
		};
		// The pictures' track opens before the clock starts, so the encoder's
		// probe holds back neither the first picture nor the sound beside it.
		if let (Some(frame), Some((_, config))) = (&picture, &video) {
			self.open(own, frame, config).await;
		}
		// A cut's play begins after the last picture already published, even
		// one cut off before it showed, so the track's time never repeats.
		let begin = begin.unwrap_or_else(|| {
			let soonest = kio::clock::now() + WRITE_LEAD;
			own.stamped
				.map_or(soonest, |stamped| soonest.max(stamped + Duration::from_millis(1)))
		});
		self.ran(play, |run| {
			run.begun.get_or_insert(begin);
		});
		let at = |timestamp: moq_net::Timestamp| begin + Duration::from(timestamp).saturating_sub(start);
		let end = std::cell::Cell::new(begin);
		let last = |until: Instant| end.set(end.get().max(until));

		let pictures = async {
			let Some((consumer, config)) = &mut video else {
				return Ok(());
			};
			let mut previous = None;
			while let Some(mut frame) = picture.take() {
				let due = at(frame.timestamp);
				ahead(due).await;
				last(due + previous.map_or(Duration::ZERO, |previous| due - previous));
				previous = Some(due);
				self.encode(own, &mut frame, config, due).await?;
				shows.queue.borrow_mut().push_back((due, play, frame));
				shows.handed.notify_one();
				picture = next_picture(consumer).await?;
			}
			Ok::<_, Error>(())
		};
		let sounds = async {
			let (Some(consumer), Some(sound)) = (&mut audio, &mut sound) else {
				return Ok(());
			};
			let bus = self.context.bus.config();
			let bytes = std::mem::size_of::<f32>() * bus.layout.channels() as usize;
			// The track's timeline runs on by its samples only from a sound
			// that starts right where the last one ended, as a loop's does
			// when its sound is its longest track: any other start, such as
			// after a loop's silence while its pictures play on, is a gap.
			let mut first = true;
			while let Some(frame) = samples.take() {
				let due = at(frame.timestamp);
				ahead(due).await;
				let length = (frame.data.len() / bytes) as u64 * 1_000_000_000 / u64::from(bus.sample_rate);
				last(due + Duration::from_nanos(length));
				let gap = std::mem::take(&mut first) && sound.ended != Some(due);
				sound.ended = Some(due + Duration::from_nanos(length));
				sound
					.writer
					.as_mut()
					.expect("a channel while playing")
					.write(due, &frame.data)
					.await?;
				if let Some(track) = &mut sound.track {
					let samples: Vec<f32> = bytemuck::pod_collect_to_vec(&frame.data);
					if let Err(err) = track.write(&samples, due, gap) {
						tracing::warn!(source = %self.name, %err, "cannot encode the source");
						self.fail_track(Status::Failed(err.to_string()));
						sound.track.take().expect("encoding").abort(&err);
					}
				}
				samples = next_samples(consumer).await?;
			}
			Ok::<_, Error>(())
		};
		tokio::try_join!(pictures, sounds)?;
		let end = end.get();
		ahead(end).await;
		Ok(end)
	}

	/// Open the source's own track for pictures like `frame`, which says their
	/// size, of the rendition `config` describes, unless it was tried already.
	async fn open(&self, own: &mut Own<'_, '_>, frame: &moq_video::Frame, config: &VideoConfig) {
		if let Some(listing) = own.listing.filter(|_| !own.opened) {
			let video = crate::publish::Video::new(listing, frame.size(), framerate(config), frame.surface.color());
			// Pictures are stamped with when they show on `own`'s timeline,
			// which may be ahead of when they encode.
			let video = video
				.await
				.and_then(|mut video| video.anchor(own.origin).map(|()| video));
			own.track = self.publish(video);
			// Only once it has, so a play cut off while it opens leaves the
			// next picture to open it.
			own.opened = true;
		}
	}

	/// Encode a played picture onto the source's own track, opened at the
	/// first picture if not before. Stamped with when it `shows`, on the
	/// timeline `own` keeps.
	async fn encode(
		&self,
		own: &mut Own<'_, '_>,
		frame: &mut moq_video::Frame,
		config: &VideoConfig,
		shows: Instant,
	) -> Result<(), Error> {
		self.open(own, frame, config).await;
		if let Some(video) = &mut own.track {
			own.stamped = Some(shows);
			let stamp = shows.saturating_duration_since(own.origin);
			frame.timestamp =
				moq_net::Timestamp::from_micros(stamp.as_micros() as u64).map_err(moq_video::Error::from)?;
			if let Err(err) = video.encode(frame) {
				self.fail_encoding(&mut own.track, err);
			}
		}
		Ok(())
	}

	/// How a replay or media source decodes pictures: from the first group,
	/// for the compositor's device.
	fn pictures(&self) -> moq_video::decode::Options {
		let mut options = moq_video::decode::Options::new();
		options.decoder.output = self.context.output;
		options.delay = REPLAY;
		options
	}

	/// How a replay or media source decodes sound: from the first group, as
	/// the bus mixes it.
	fn sounds(&self) -> moq_audio::decode::Options {
		let bus = self.context.bus.config();
		let mut options = moq_audio::decode::Options::new();
		options.delay = REPLAY;
		options.output.format = moq_audio::Format::F32;
		options.output.sample_rate = Some(bus.sample_rate);
		options.output.layout = Some(bus.layout);
		options
	}

	/// A writer into the bus for decoded sound, as [`sounds`](Self::sounds)
	/// and broadcasts decode it, through `strip`.
	fn writer(&self, strip: Strip) -> Result<Writer, Error> {
		let bus = self.context.bus.config();
		let mut input = moq_audio::encode::Input::new(bus.sample_rate, bus.layout);
		input.format = moq_audio::Format::F32;
		let channel = self.context.bus.positioned(input, strip)?;
		Ok(Writer::new(
			channel,
			self.context.mixed.clone(),
			bus.sample_rate,
			bus.layout.channels(),
		))
	}
}

/// A media source's sound: the bus channel it plays into, taken anew on a
/// restart, and its own track.
struct Sound<'a> {
	writer: Option<Writer>,
	track: Option<crate::publish::Audio<'a>>,
	/// When the last sound written ends, or `None` after a break, such as a
	/// restart, so the next sound's track starts its timeline afresh.
	ended: Option<Instant>,
}

/// A media source's pictures, handed over [`WRITE_LEAD`] ahead of their moments
/// with their plays, and woken whenever one is.
#[derive(Default)]
struct Shows {
	queue: RefCell<VecDeque<(Instant, u64, moq_video::Frame)>>,
	handed: Notify,
}

/// A played source's own track, opened at its first picture, and the timeline
/// its pictures are stamped on: from `origin`, when they show.
struct Own<'a, 'b> {
	listing: Option<&'a Listing<'b>>,
	track: Option<crate::publish::Video<'b>>,
	/// Whether the track was opened, or failed to, so it is tried once.
	opened: bool,
	origin: Instant,
	/// When the last picture encoded onto the track shows.
	stamped: Option<Instant>,
}

impl<'a, 'b> Own<'a, 'b> {
	fn new(listing: Option<&'a Listing<'b>>, origin: Instant) -> Self {
		Self {
			listing,
			track: None,
			opened: false,
			origin,
			stamped: None,
		}
	}
}

/// Wait to hand over what is due at `due`: [`WRITE_LEAD`] ahead, so a sound
/// written then is mixed whole when due. One due at once is handed over without a
/// timer, which the timer wheel would round up to its next millisecond, a
/// moment too late.
async fn ahead(due: Instant) {
	let at = due - WRITE_LEAD;
	if at > kio::clock::now() {
		kio::clock::sleep_until(at).await;
	}
}

/// The next picture `consumer` decodes, passing over gaps, or `None` at the
/// end of its track.
async fn next_picture(consumer: &mut moq_video::decode::Consumer) -> Result<Option<moq_video::Frame>, Error> {
	while let Some(read) = consumer.read().await? {
		if let moq_video::decode::Read::Frame(frame) = read {
			return Ok(Some(frame));
		}
	}
	Ok(None)
}

/// The next sound `consumer` decodes, passing over gaps, or `None` at the end
/// of its track.
async fn next_samples(consumer: &mut moq_audio::decode::Consumer) -> Result<Option<moq_audio::Frame>, Error> {
	while let Some(read) = consumer.read().await? {
		if let moq_audio::decode::Read::Frame(frame) = read {
			return Ok(Some(frame));
		}
	}
	Ok(None)
}

/// The frame rate a rendition's own track is encoded at: its catalog's, or 30
/// fps when it names none.
fn framerate(config: &VideoConfig) -> moq_video::FrameRate {
	config
		.framerate
		.filter(|rate| rate.is_finite() && *rate >= 1.0)
		.and_then(|rate| moq_video::FrameRate::new(rate.round() as u32, 1).ok())
		.unwrap_or_else(|| moq_video::FrameRate::new(30, 1).expect("a valid frame rate"))
}

/// How a replay or media source reads its broadcast: from the first group,
/// where its tracks declare they start, skipping nothing as late, since all of
/// it is history.
const REPLAY: moq_net::track::Delay = moq_net::track::Delay {
	min: Some(Duration::ZERO),
	max: None,
};

/// The best enabled rendition the broadcast serves itself, of `renditions`
/// ranked best first.
///
/// One served from another broadcast would need that broadcast's own
/// subscription, which a source does not open, so a catalog offering only
/// those is refused rather than shown as black or played as silence.
pub(crate) fn pick<'a, C: Reference + Clone + 'a>(
	renditions: impl Iterator<Item = (&'a String, &'a C)>,
) -> Result<Option<(String, C)>, Error> {
	let mut foreign = None;
	for (id, config) in renditions {
		if config.broadcast().is_none() {
			return Ok(Some((id.clone(), config.clone())));
		}
		foreign.get_or_insert(id);
	}
	match foreign {
		Some(id) => Err(Error::Unsupported(format!(
			"rendition {id} is served from another broadcast"
		))),
		None => Ok(None),
	}
}

/// Start the feed for `source`, and the driver that fills it, `held` from
/// playing a media file until the first [`Feed::play`]. A layer `follows` its
/// broadcast's path and the clock of the broadcast source playing it.
fn open(
	name: &str,
	source: &Source,
	context: &Context,
	held: bool,
	follows: Option<(String, timeline::Consumer)>,
) -> (Feed, Driver) {
	let picture = Mailbox::default();
	let delivered = Delivered::default();
	let strip = source.has_audio().then(Strip::default);
	let status = kio::Producer::new(Status::Starting);
	let page = matches!(source, Source::Browser { .. }).then(Page::default);
	// Claimed here, in the order sources open, so an edited source's
	// replacement owns the listing however their drivers are scheduled.
	let track = context
		.publish
		.clone()
		.filter(|_| source.publishes())
		.map(|publish| Track {
			claim: publish.claim(name),
			publish,
			status: kio::Producer::new(Status::Starting),
		});

	let played = Playback::default();
	let clock = matches!(source, Source::Broadcast { .. }).then(|| timeline::Producer::new(VIDEO_DELAY));
	let (plays, replays) = match source {
		Source::Replay => {
			let (plays, replays) = watch::channel(Next::default());
			(Some(plays), Some(replays))
		}
		_ => (None, None),
	};
	let (cues, cued) = match source {
		Source::Media { .. } => {
			let (cues, cued) = watch::channel(Cue { play: 0, stopped: held });
			(Some(cues), Some(cued))
		}
		_ => (None, None),
	};
	let silent = Arc::<AtomicBool>::default();
	let runs = Runs::new(Mutex::new(Latest {
		run: Run::default(),
		stopped: held,
	}));

	let feed = Feed {
		source: source.clone(),
		picture: picture.clone(),
		delivered: delivered.clone(),
		strip: strip.clone(),
		status: status.consume(),
		page: page.clone(),
		track: track.as_ref().map(|track| track.status.consume()),
		played: played.clone(),
		plays,
		cues,
		runs: runs.clone(),
		silent: silent.clone(),
		clock: match &follows {
			Some((_, schedule)) => Some(schedule.clone()),
			None => clock.as_ref().map(timeline::Producer::consume),
		},
	};
	let driver = Driver {
		name: name.to_string(),
		source: source.clone(),
		context: context.clone(),
		picture,
		delivered,
		strip,
		status,
		page,
		track,
		played,
		plays: replays,
		cues: cued,
		runs,
		silent,
		clock,
		follows,
	};
	(feed, driver)
}

impl Source {
	/// Whether the source publishes its own track: a picture the studio captures or
	/// loads itself, before the scene's filters, or the sound of a microphone,
	/// system audio, or media file as captured or decoded, before its gain and
	/// mute.
	///
	/// A broadcast, or a layer of one, is already published by its own
	/// publisher, and a clone shows its source's track, its filters carried by
	/// the layout rather than encoded again. Web pages and overlays are seen
	/// only in the program, since H.264 would encode their transparency over
	/// black, a LUT is looked up, never shown, and a scene is drawn from other
	/// sources.
	pub fn publishes(&self) -> bool {
		match self {
			Source::Camera { .. }
			| Source::Display { .. }
			| Source::Window { .. }
			| Source::Microphone { .. }
			| Source::SystemAudio
			| Source::Color { .. }
			| Source::Image { .. }
			| Source::Media { .. }
			| Source::Replay => true,
			Source::Broadcast { .. }
			| Source::Layer { .. }
			| Source::Browser { .. }
			| Source::Overlay { .. }
			| Source::Clone { .. }
			| Source::Lut { .. }
			| Source::Scene { .. }
			| Source::Program => false,
		}
	}
}

/// The frame rate a capture is encoded at when the device reports none.
#[cfg(feature = "capture")]
fn default_framerate() -> moq_video::FrameRate {
	moq_video::FrameRate::new(30, 1).expect("a valid frame rate")
}

fn set(status: &kio::Producer<Status>, value: Status) {
	if let Ok(mut status) = status.write() {
		*status = value;
	}
}

/// Open `source` in a layout the bus can mix. Past stereo a device's channels
/// carry no speaker positions to downmix by, such as a microphone array's or an
/// interface's inputs, so a device that defaults to more is asked for stereo,
/// then mono, and fails with the formats it offers if it has neither.
#[cfg(feature = "capture")]
async fn capture(source: moq_audio::capture::Source) -> Result<moq_audio::capture::Producer, moq_audio::Error> {
	let mut config = moq_audio::capture::Config::default();
	config.source = source;
	let capture = moq_audio::capture::Producer::open(config.clone()).await?;
	if !matches!(capture.consume().input().layout, moq_audio::Layout::Discrete(_)) {
		return Ok(capture);
	}
	drop(capture);
	config.channels = Some(2);
	match moq_audio::capture::Producer::open(config.clone()).await {
		Err(moq_audio::Error::Unsupported(_)) => {
			config.channels = Some(1);
			moq_audio::capture::Producer::open(config).await
		}
		result => result,
	}
}

/// Read a still image, decoded to straight-alpha RGBA, and the file it came from.
fn image(path: &Path) -> Result<(Picture, bytes::Bytes), Error> {
	let bytes = std::fs::read(path).map_err(|err| Error::Image(format!("{}: {err}", path.display())))?;
	let picture = decode(&bytes).map_err(|err| Error::Image(format!("{}: {err}", path.display())))?;
	Ok((picture, bytes.into()))
}

/// A PNG or JPEG file's picture, within the image crate's default limits.
fn decode(bytes: &[u8]) -> Result<Picture, image::ImageError> {
	let image = image::load_from_memory(bytes)?.into_rgba8();
	Ok(Picture::Rgba {
		size: moq_video::Size::new(image.width(), image.height()),
		pixels: image.into_raw(),
	})
}

/// Helpers for tests across the crate.
#[cfg(test)]
pub(crate) mod test {
	use super::*;

	/// A context with no relay or device behind it, so broadcasts stay offline
	/// while the returned origin lives, whose bus starts now. Call from a
	/// [`tokio::task::LocalSet`].
	pub(crate) fn context(dir: &Path) -> (Context, moq_net::origin::Producer) {
		let (origin, driver) = moq_net::origin::Producer::new(Default::default());
		tokio::task::spawn_local(moq_net::time::run(driver));
		let (bus, driver) = Bus::new(Default::default()).unwrap();
		let (_, mixed) = Mixer::new(driver, kio::clock::now());
		let context = Context {
			bus,
			mixed,
			origin: origin.consume(),
			dir: dir.to_path_buf(),
			output: moq_video::Output::Cpu,
			browser: None,
			publish: None,
		};
		(context, origin)
	}

	/// Run `drivers` on the local task set, and wait until every source in
	/// `sources` has left [`Status::Starting`].
	pub(crate) async fn start(sources: &Sources, drivers: Vec<Driver>) {
		for driver in drivers {
			tokio::task::spawn_local(driver.run());
		}
		for _ in 0..1000 {
			if sources.iter().all(|(_, feed)| feed.status() != Status::Starting) {
				return;
			}
			tokio::task::yield_now().await;
		}
		panic!("sources never started");
	}

	/// Three seconds of 30 fps H.264 with a keyframe each second, but for the
	/// second `skip` the broadcast paused through, its track open for as long
	/// as the returned producer lives, as a live one is.
	pub(crate) async fn video(
		skip: Option<u64>,
	) -> (
		moq_net::broadcast::Producer,
		(String, VideoConfig),
		moq_video::encode::Producer,
	) {
		let mut broadcast = moq_net::broadcast::Info::new().produce();
		let catalog = moq_mux::catalog::Producer::new(&mut broadcast, Default::default()).unwrap();
		let mut config = moq_video::encode::Config::new(64, 64, moq_video::FrameRate::new(30, 1).unwrap());
		config.kind = moq_video::encode::Kind::Software;
		let rendition = config.probe().await.unwrap();
		let reserved = catalog.reserve().with_interval(config.effective_join());
		let mut producer = moq_video::encode::Producer::new(broadcast.clone(), reserved, rendition).unwrap();
		let mut encoder = moq_video::encode::Encoder::new(&config).unwrap();
		let gray = vec![0x80; 64 * 64 * 4];
		for index in 0..90u64 {
			if skip == Some(index / 30) {
				if index % 30 == 0 {
					producer.discontinuity().unwrap();
				}
				continue;
			}
			if index % 30 == 0 {
				encoder.cut().unwrap();
			}
			let surface = moq_video::Surface::rgba(&gray, moq_video::Size::new(64, 64)).unwrap();
			let frame = moq_video::Frame::new(
				surface,
				moq_net::Timestamp::from_micros(index * 1_000_000 / 30).unwrap(),
			);
			producer.publish(&encoder.encode(&frame).unwrap()).unwrap();
		}
		let video = catalog.snapshot().video.renditions.into_iter().next().unwrap();
		(broadcast, video, producer)
	}

	/// Wait until `f` returns something, without letting a paused clock move.
	///
	/// Decoding and probing an encoder run on moq-video's own threads off Apple
	/// platforms, which tokio cannot see: a runtime left idle while one works
	/// advances a paused clock to its earliest timer, which may lie past whatever
	/// the test was waiting for. Keeping a timer due now holds the clock where it
	/// is, but only on a whole millisecond: the timer wheel rounds a deadline up
	/// to one, so from between two a zero sleep is due at the next, and the idle
	/// runtime moves the clock a millisecond per loop. Move it with
	/// [`until_time`], which lands on one, never `tokio::time::advance`, which
	/// may not. A wait that never ends is a hang nextest kills.
	pub(crate) async fn until<T>(mut f: impl FnMut() -> Option<T>) -> T {
		loop {
			if let Some(value) = f() {
				return value;
			}
			// Through the timer driver, so the replay's due timers fire, but
			// due now, so a paused clock has nowhere later to jump to.
			#[expect(clippy::disallowed_methods, reason = "a zero sleep that holds the paused clock")]
			tokio::time::sleep(Duration::ZERO).await;
		}
	}

	/// Move a paused clock to `at`, rounded up to its millisecond, and no
	/// further: the timer due then is the earliest pending while the runtime
	/// waits for it. See [`until`].
	pub(crate) async fn until_time(at: Instant) {
		kio::clock::sleep_until(at).await;
	}

	/// Mix `mixer` along with the clock from `at` to `until`, 10 ms at a time,
	/// each block once the clock has passed it, returning the loudest `mix`
	/// got. A bus started at `at` mixes on the clock as the program's does.
	pub(crate) async fn listen(mixer: &mut Mixer, mix: &Mix, at: &mut Instant, until: Instant) -> f32 {
		const STEP: Duration = Duration::from_millis(10);
		let frames =
			(STEP.as_nanos() * u128::from(moq_audio::bus::Config::default().sample_rate) / 1_000_000_000) as usize;
		mix.level();
		let mut loudest = 0.0f32;
		while *at < until {
			*at += STEP;
			until_time(*at).await;
			mixer.advance(frames);
			loudest = loudest.max(mix.level().peak());
		}
		loudest
	}

	/// Run `f` on a local task set, as drivers need.
	pub(crate) async fn local<F: Future>(f: F) -> F::Output {
		tokio::task::LocalSet::new().run_until(f).await
	}
}

#[cfg(test)]
mod tests {
	use super::test::{context, local, start, until, until_time, video};
	use super::*;
	use crate::collection::{Audio, Rgba};

	fn collection() -> Collection {
		let mut collection = Collection::new();
		collection.sources.insert(
			"red".into(),
			Source::Color {
				color: Rgba([255, 0, 0, 255]),
			},
		);
		collection.sources.insert(
			"guest".into(),
			Source::Broadcast {
				path: "room/guest".into(),
			},
		);
		collection.audio_scenes[0].sounds.insert("guest".into());
		collection
	}

	/// A picture that replaces a clear before the compositor takes it still
	/// says the source cleared, once.
	#[test]
	fn a_clear_outlives_the_picture_after_it() {
		let mut slot = Slot::default();
		slot.show(clear());
		assert!(!slot.take().unwrap().cleared);
		slot.clear();
		slot.show(Picture::Rgba {
			size: moq_video::Size::new(2, 1),
			pixels: vec![255; 8],
		});
		let taken = slot.take().unwrap();
		assert!(taken.cleared);
		assert!(matches!(taken.picture, Picture::Rgba { size, .. } if size == moq_video::Size::new(2, 1)));
		assert!(slot.take().is_none());
		slot.show(clear());
		assert!(!slot.take().unwrap().cleared);
	}

	#[tokio::test]
	async fn a_color_shows_one_pixel() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection());
			assert_eq!(drivers.len(), 2);
			start(&sources, drivers).await;

			let red = sources.get("red").unwrap();
			assert_eq!(red.status(), Status::Live);
			assert!(red.strip().is_none(), "a color is silent");
			let Some(Picture::Rgba { size, pixels }) = red.next_picture() else {
				panic!("a color shows a picture");
			};
			assert_eq!(size, moq_video::Size::new(1, 1));
			assert_eq!(pixels, [255, 0, 0, 255]);
			assert!(red.next_picture().is_none(), "taken once");

			let guest = sources.get("guest").unwrap();
			assert_eq!(guest.status(), Status::Offline, "nothing is announced");
			assert!(guest.strip().is_some(), "a broadcast feeds the bus");
		})
		.await;
	}

	/// A camera or broadcast that stops delivering pictures, or a microphone or
	/// system audio that stops delivering sound, reads as stalled once [`STALL`]
	/// passes; a display, a still, and a source that never delivered do not.
	#[tokio::test]
	async fn a_live_source_without_new_deliveries_stalls() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let start = kio::clock::now();
			let opened = |source: Source| {
				let (feed, driver) = open("source", &source, &context, false, None);
				set(&driver.status, Status::Live);
				(feed, driver)
			};

			let (camera, driver) = opened(Source::Camera { id: None });
			assert_eq!(camera.stalled(start + STALL), None, "no picture yet");
			*driver.delivered.lock().unwrap() = Some(start);
			assert_eq!(camera.stalled(start + STALL / 2), None);
			assert_eq!(camera.stalled(start + STALL * 3), Some(Stall::Picture(STALL * 3)));
			set(&driver.status, Status::Failed("unplugged".into()));
			assert_eq!(camera.stalled(start + STALL * 3), None, "a failure says why on its own");

			for source in [Source::Microphone { id: None }, Source::SystemAudio] {
				let (audio, driver) = opened(source);
				assert_eq!(audio.stalled(start + STALL), None, "no sound yet");
				*driver.delivered.lock().unwrap() = Some(start);
				assert_eq!(audio.stalled(start + STALL / 2), None);
				assert_eq!(audio.stalled(start + STALL * 3), Some(Stall::Sound(STALL * 3)));
			}

			let (display, driver) = opened(Source::Display { id: None });
			*driver.delivered.lock().unwrap() = Some(start);
			assert_eq!(display.stalled(start + STALL * 3), None, "a still screen sends nothing");
		})
		.await;
	}

	/// An audio scene plays a source with sound that it reaches and that is
	/// not muted: one it holds, or one an audio scene it holds holds, once.
	#[tokio::test]
	async fn an_audio_scene_plays_only_what_it_reaches() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let mut collection = collection();
			let program = crate::AudioScene::DEFAULT;
			collection.audio_scenes.push(crate::AudioScene {
				scenes: [program.to_string()].into(),
				..crate::AudioScene::new("Headphones")
			});
			collection.audio_scenes.push(crate::AudioScene::new("Music"));
			collection.validate().unwrap();
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			assert!(sources.plays(program, "guest"));
			assert!(sources.plays("Headphones", "guest"), "through the one it holds");
			assert!(!sources.plays("Music", "guest"), "held by none it reaches");
			assert!(!sources.plays(program, "red"), "a color is silent");
			assert!(!sources.plays(program, "nobody"));
			assert!(!sources.plays("Nothing", "guest"));

			// A restart, such as a page's reload, leaves every mix at once, and
			// the source comes back in under its new strip.
			let before = sources.get("guest").unwrap().strip().unwrap().clone();
			sources.restart("guest");
			for scene in [program, "Headphones"] {
				assert!(
					!sources.mix(scene).unwrap().routes(&before),
					"{scene} keeps the old strip"
				);
			}
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			assert!(sources.plays(program, "guest"));

			// Held directly too, it still plays once.
			collection.audio_scenes[1].sounds.insert("guest".into());
			collection.audio_scenes[0].sounds.clear();
			assert!(sources.sync(&collection).is_empty());
			assert!(!sources.plays(program, "guest"), "left out of the program");
			assert!(sources.plays("Headphones", "guest"));

			let muted = Audio {
				muted: true,
				..Audio::default()
			};
			collection.audio.insert("guest".into(), muted);
			assert!(sources.sync(&collection).is_empty());
			assert!(!sources.plays("Headphones", "guest"), "muted everywhere");
		})
		.await;
	}

	/// Editing a source restarts it, removing one stops it, and an untouched one
	/// keeps running. Gain, pan, delay, and routing changes reach the strip without
	/// restarting anything, and a source plays in every audio scene holding it
	/// until it is removed.
	#[tokio::test]
	async fn sync_restarts_only_what_changed() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let mut collection = collection();
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			let guest = sources.get("guest").unwrap().strip().unwrap().clone();
			let program = sources.mix(crate::AudioScene::DEFAULT).unwrap().clone();
			assert!(program.routes(&guest));

			collection.audio_scenes.push(crate::AudioScene::new("Stream"));
			collection.audio.insert(
				"guest".into(),
				Audio {
					gain: 0.25,
					muted: true,
					pan: -0.5,
					delay: Duration::from_millis(120),
				},
			);
			assert!(sources.sync(&collection).is_empty());
			assert_eq!(guest.gain(), 0.25);
			assert!(guest.muted());
			assert_eq!(guest.pan(), -0.5);
			assert_eq!(guest.delay(), Duration::from_millis(120));
			let stream = sources.mix("Stream").unwrap().clone();
			assert!(program.routes(&guest));
			assert!(!stream.routes(&guest), "not held by the new audio scene");

			collection.audio_scenes[1].sounds.insert("guest".into());
			assert!(sources.sync(&collection).is_empty());
			assert!(stream.routes(&guest), "held");

			collection.audio_scenes.retain(|scene| scene.name != "Stream");
			assert!(sources.sync(&collection).is_empty());
			assert!(sources.mix("Stream").is_none());

			collection.sources.insert(
				"red".into(),
				Source::Color {
					color: Rgba([0, 0, 255, 255]),
				},
			);
			let drivers = sources.sync(&collection);
			assert_eq!(drivers.iter().map(Driver::name).collect::<Vec<_>>(), ["red"]);
			start(&sources, drivers).await;
			let Some(Picture::Rgba { pixels, .. }) = sources.get("red").unwrap().next_picture() else {
				panic!("the edited color shows");
			};
			assert_eq!(pixels, [0, 0, 255, 255]);

			collection.sources.remove("red");
			assert!(sources.sync(&collection).is_empty());
			assert!(sources.get("red").is_none());

			collection.sources.remove("guest");
			collection.audio_scenes[0].sounds.clear();
			assert!(sources.sync(&collection).is_empty());
			assert!(!program.routes(&guest), "a removed source leaves at once");
		})
		.await;
	}

	/// A broadcast source follows catalog updates: when a better rendition
	/// appears, it switches to it without the broadcast restarting.
	#[tokio::test]
	async fn a_broadcast_follows_its_catalog() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, origin) = context(dir.path());
			let publication = origin
				.route("room/guest", moq_net::origin::RouteOptions::hidden())
				.unwrap();
			let mut broadcast = publication.broadcast().clone();
			let catalog = moq_mux::catalog::Producer::new(&mut broadcast, Default::default()).unwrap();
			// A known bitrate ranks a rendition above one without.
			let audio = |name: &str, bitrate: Option<u64>| {
				let track = broadcast
					.create_track(name, catalog.track_info(hang::catalog::PRIORITY.audio))
					.unwrap();
				let mut config = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Opus, 48_000, 2);
				config.bitrate = bitrate;
				catalog
					.audio(
						track,
						moq_mux::catalog::hang::Container::Legacy(moq_mux::container::Kind::Audio),
						config,
					)
					.unwrap()
			};
			let first = audio("first", None);
			publication.announce().unwrap();

			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection());
			start(&sources, drivers).await;

			let mut first = first.demand();
			while first.wanted_changed().await.unwrap().is_none() {}

			let second = audio("second", Some(128_000));
			let mut second = second.demand();
			while second.wanted_changed().await.unwrap().is_none() {}
			while first.wanted_changed().await.unwrap().is_some() {}
			assert_eq!(sources.get("guest").unwrap().status(), Status::Live);
		})
		.await;
	}

	/// A rendition that keeps its catalog ID but points at a replacement track,
	/// as the JS publisher does on a codec change, moves playback to that track.
	#[tokio::test]
	async fn a_broadcast_follows_a_renditions_track() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, origin) = context(dir.path());
			let publication = origin
				.route("room/guest", moq_net::origin::RouteOptions::hidden())
				.unwrap();
			let mut broadcast = publication.broadcast().clone();
			let mut catalog = moq_mux::catalog::Producer::new(&mut broadcast, Default::default()).unwrap();
			let info = catalog.track_info(hang::catalog::PRIORITY.audio);
			let first = broadcast.create_track("first", info.clone()).unwrap();
			let second = broadcast.create_track("second", info).unwrap();
			let point = |catalog: &mut moq_mux::catalog::Producer, track: &str| {
				let mut config = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Opus, 48_000, 2);
				config.track = Some(track.into());
				catalog
					.mutate(|catalog| {
						catalog.audio.renditions.insert("audio".into(), config);
					})
					.unwrap();
			};
			point(&mut catalog, "first");
			publication.announce().unwrap();

			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection());
			start(&sources, drivers).await;

			let mut first = first.demand();
			while first.wanted_changed().await.unwrap().is_none() {}

			point(&mut catalog, "second");
			let mut second = second.demand();
			while second.wanted_changed().await.unwrap().is_none() {}
			while first.wanted_changed().await.unwrap().is_some() {}
			assert_eq!(sources.get("guest").unwrap().status(), Status::Live);
		})
		.await;
	}

	/// A catalog whose only rendition lives in another broadcast fails the
	/// source rather than leaving it live and silent.
	#[tokio::test]
	async fn a_foreign_rendition_is_refused() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, origin) = context(dir.path());
			let publication = origin
				.route("room/guest", moq_net::origin::RouteOptions::hidden())
				.unwrap();
			let mut broadcast = publication.broadcast().clone();
			let mut catalog = moq_mux::catalog::Producer::new(&mut broadcast, Default::default()).unwrap();
			let mut config = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Opus, 48_000, 2);
			config.broadcast = Some(moq_net::path::Relative::new("../other").unwrap());
			catalog
				.mutate(|catalog| {
					catalog.audio.renditions.insert("audio".into(), config);
				})
				.unwrap();
			publication.announce().unwrap();

			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection());
			start(&sources, drivers).await;

			let guest = sources.get("guest").unwrap();
			for _ in 0..1000 {
				if let Status::Failed(reason) = guest.status() {
					assert!(reason.contains("another broadcast"), "{reason}");
					return;
				}
				tokio::task::yield_now().await;
			}
			panic!("the source never failed: {:?}", guest.status());
		})
		.await;
	}

	/// A live broadcast at `room/guest`, writing nothing until the test does.
	struct Live {
		_publication: moq_net::origin::Route,
		broadcast: moq_net::broadcast::Producer,
		catalog: moq_mux::catalog::Producer,
		video: moq_video::encode::Producer,
		encoder: moq_video::encode::Encoder,
		audio: Option<moq_audio::encode::Producer>,
	}

	/// A video track on its own timestamp base.
	async fn rendition<E: moq_mux::catalog::hang::CatalogExt>(
		broadcast: &moq_net::broadcast::Producer,
		timebase: moq_mux::catalog::Timebase<E>,
	) -> (moq_video::encode::Producer, moq_video::encode::Encoder) {
		let mut config = moq_video::encode::Config::new(64, 64, moq_video::FrameRate::new(30, 1).unwrap());
		config.kind = moq_video::encode::Kind::Software;
		let rendition = config.probe().await.unwrap();
		let reserved = timebase.reserve().with_interval(config.effective_join());
		let video = moq_video::encode::Producer::new(broadcast.clone(), reserved, rendition).unwrap();
		(video, moq_video::encode::Encoder::new(&config).unwrap())
	}

	/// A broadcast with a video track, and an audio one with `audio`.
	async fn live(origin: &moq_net::origin::Producer, audio: bool) -> Live {
		let publication = origin
			.route("room/guest", moq_net::origin::RouteOptions::hidden())
			.unwrap();
		let mut broadcast = publication.broadcast().clone();
		let catalog = moq_mux::catalog::Producer::new(&mut broadcast, Default::default()).unwrap();
		let (video, encoder) = rendition(&broadcast, catalog.timebase()).await;
		let audio = audio.then(|| {
			let input = moq_audio::encode::Input::default();
			let mut options = moq_audio::encode::Options::default();
			options.config = moq_audio::encode::Config::from_input(moq_audio::encode::Codec::Pcm, &input);
			moq_audio::encode::Producer::new(&mut broadcast, catalog.clone(), input, &options).unwrap()
		});
		publication.announce().unwrap();
		Live {
			_publication: publication,
			broadcast,
			catalog,
			video,
			encoder,
			audio,
		}
	}

	/// Encode and publish one gray picture stamped `at`.
	fn picture(producer: &mut moq_video::encode::Producer, encoder: &mut moq_video::encode::Encoder, at: u64) {
		let gray = vec![0x80; 64 * 64 * 4];
		let surface = moq_video::Surface::rgba(&gray, moq_video::Size::new(64, 64)).unwrap();
		let frame = moq_video::Frame::new(surface, moq_net::Timestamp::from_millis(at).unwrap());
		producer.publish(&encoder.encode(&frame).unwrap()).unwrap();
	}

	/// The picture a feed shows next, once it shows one.
	async fn shown(feed: &Feed) -> Duration {
		until(|| match feed.next_picture() {
			Some(Picture::Frame(frame)) => Some(frame.timestamp.into()),
			Some(_) => panic!("a broadcast shows only frames"),
			None => None,
		})
		.await
	}

	/// Without audio to follow, a broadcast's picture shows [`VIDEO_DELAY`]
	/// after it arrives, not on arrival.
	#[tokio::test(start_paused = true)]
	async fn a_picture_waits_for_its_moment() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, origin) = context(dir.path());
			let mut live = live(&origin, false).await;
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection());
			start(&sources, drivers).await;
			let feed = sources.get("guest").unwrap();
			let clock = feed.clock.clone().unwrap();

			until_time(kio::clock::now()).await;
			let arrived = kio::clock::now();
			picture(&mut live.video, &mut live.encoder, 1_000);
			let stamp = moq_net::Timestamp::from_millis(1_000).unwrap();
			let due = until(|| clock.due(stamp)).await;
			assert_eq!(due, arrived + VIDEO_DELAY);
			assert!(feed.next_picture().is_none(), "shown on arrival");
			until_time(due).await;
			assert_eq!(shown(feed).await, Duration::from(stamp));
			assert_eq!(feed.delay(), Some(VIDEO_DELAY));
		})
		.await;
	}

	/// A layer shows each picture when its broadcast source's clock says, as
	/// the program's picture stamped alike shows, not its own delay after it
	/// arrives.
	#[tokio::test(start_paused = true)]
	async fn a_layer_plays_on_its_broadcasts_clock() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, origin) = context(dir.path());
			let publication = origin
				.route("room/guest", moq_net::origin::RouteOptions::hidden())
				.unwrap();
			let mut broadcast = publication.broadcast().clone();
			let config = moq_mux::catalog::Config::default().with_catalog(Default::default());
			let mut catalog = crate::publish::Catalog::new(&mut broadcast, config).unwrap();
			let (mut program, mut encoder) = rendition(&broadcast, catalog.timebase()).await;
			// The camera's track is in the program's broadcast, listed under its
			// name in the catalog's own section rather than as the program's.
			let mut local = moq_net::broadcast::Info::new().produce();
			let listing = moq_mux::catalog::Producer::new(&mut local, Default::default()).unwrap();
			let (mut cam, mut cam_encoder) = rendition(&broadcast, listing.timebase()).await;
			publication.announce().unwrap();

			let mut collection = collection();
			collection.sources.insert(
				"cam".into(),
				Source::Layer {
					broadcast: "guest".into(),
					source: "cam".into(),
				},
			);
			collection.validate().unwrap();
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			let layer = sources.get("cam").unwrap();
			let clock = sources.get("guest").unwrap().clock.clone().unwrap();

			until_time(kio::clock::now()).await;
			let arrived = kio::clock::now();
			picture(&mut program, &mut encoder, 1_000);
			let stamp = moq_net::Timestamp::from_millis(1_000).unwrap();
			let due = until(|| clock.due(stamp)).await;
			assert_eq!(due, arrived + VIDEO_DELAY);

			// Half the delay later, so on a clock of its own it would show late.
			until_time(arrived + VIDEO_DELAY / 2).await;
			picture(&mut cam, &mut cam_encoder, 1_000);
			let listed = until(|| Some(listing.snapshot()).filter(|listed| !listed.video.renditions.is_empty())).await;
			catalog
				.mutate(|root| {
					root.ext.studio.sources.insert("cam".into(), listed);
				})
				.unwrap();
			until_time(due - Duration::from_millis(1)).await;
			assert!(layer.next_picture().is_none(), "shown before its moment");
			until_time(due).await;
			assert_eq!(shown(layer).await, Duration::from(stamp));
			assert_eq!(layer.delay(), Some(VIDEO_DELAY), "the program's");
		})
		.await;
	}

	/// A layer whose source is a still, an image file on a binary track,
	/// shows it as it arrives.
	#[tokio::test]
	async fn a_layer_shows_a_still() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, origin) = context(dir.path());
			let publication = origin
				.route("room/guest", moq_net::origin::RouteOptions::hidden())
				.unwrap();
			let mut broadcast = publication.broadcast().clone();
			let config = moq_mux::catalog::Config::default().with_catalog(Default::default());
			let mut catalog = crate::publish::Catalog::new(&mut broadcast, config).unwrap();
			let mut track = broadcast.create_track("source/logo.png", None).unwrap();
			let pixel = image::RgbaImage::from_raw(1, 1, vec![255, 0, 0, 255]).unwrap();
			let mut png = std::io::Cursor::new(Vec::new());
			pixel.write_to(&mut png, image::ImageFormat::Png).unwrap();
			track.write_frame(None, bytes::Bytes::from(png.into_inner())).unwrap();
			let mut still = hang::catalog::BinaryConfig::new(hang::catalog::Mode::Snapshot);
			still.mime = Some("image/png".into());
			let mut logo = hang::Catalog::default();
			logo.binary.tracks.insert("source/logo.png".into(), still);
			catalog
				.mutate(|root| {
					root.ext.studio.sources.insert("logo".into(), logo);
				})
				.unwrap();
			publication.announce().unwrap();

			let mut collection = collection();
			collection.sources.insert(
				"logo".into(),
				Source::Layer {
					broadcast: "guest".into(),
					source: "logo".into(),
				},
			);
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			let layer = sources.get("logo").unwrap();
			let pixels = until(|| match layer.next_picture() {
				Some(Picture::Rgba { pixels, .. }) => Some(pixels),
				_ => None,
			})
			.await;
			assert_eq!(pixels, [255, 0, 0, 255]);
		})
		.await;
	}

	/// A layer stops with its broadcast source, and starts again on its
	/// replacement's clock.
	#[tokio::test]
	async fn a_layer_follows_its_broadcast_sources_replacement() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut collection = collection();
			collection.sources.insert(
				"cam".into(),
				Source::Layer {
					broadcast: "guest".into(),
					source: "cam".into(),
				},
			);
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			assert_eq!(drivers.len(), 3);
			start(&sources, drivers).await;
			collection.sources.insert(
				"guest".into(),
				Source::Broadcast {
					path: "room/other".into(),
				},
			);
			let mut names: Vec<String> = sources
				.sync(&collection)
				.iter()
				.map(|driver| driver.name().to_string())
				.collect();
			names.sort();
			assert_eq!(names, ["cam", "guest"]);
		})
		.await;
	}

	/// A video track replaced by one stamped earlier starts its own delay over,
	/// rather than showing every picture on arrival against the old track's.
	#[tokio::test(start_paused = true)]
	async fn a_replacement_track_keeps_its_delay() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, origin) = context(dir.path());
			let mut live = live(&origin, false).await;
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection());
			start(&sources, drivers).await;
			let feed = sources.get("guest").unwrap();
			let clock = feed.clock.clone().unwrap();
			picture(&mut live.video, &mut live.encoder, 1_000);
			let first = moq_net::Timestamp::from_millis(1_000).unwrap();
			let due = until(|| clock.due(first)).await;
			until_time(due).await;
			assert_eq!(shown(feed).await, Duration::from(first));

			// Its stamps sit half a second before the first track's clock reading.
			let timebase = live.catalog.timebase();
			timebase
				.place(
					moq_net::Timestamp::ZERO,
					kio::clock::wall() - Duration::from_millis(500),
				)
				.unwrap();
			let (video, mut encoder) = rendition(&live.broadcast, timebase).await;
			// Finished before it goes, as a publisher retires a rendition: one
			// dropped unfinished aborts its track, which fails the source if
			// that reaches it before the catalog naming the replacement does.
			let mut old = std::mem::replace(&mut live.video, video);
			old.finish().unwrap();
			drop(old);
			until_time(kio::clock::now()).await;
			let arrived = kio::clock::now();
			picture(&mut live.video, &mut encoder, 0);
			let moved = until(|| match feed.next_picture() {
				Some(Picture::Frame(frame)) => panic!("{:?} shown on arrival", frame.timestamp),
				_ => clock.due(first).filter(|at| *at != due),
			})
			.await;
			assert!(moved > due, "the replacement re-anchored behind the first track");
			until_time(arrived + VIDEO_DELAY - Duration::from_millis(1)).await;
			assert!(feed.next_picture().is_none(), "shown before its delay");
			until_time(arrived + VIDEO_DELAY).await;
			assert!(
				shown(feed).await < Duration::from(first),
				"stamped before the first track"
			);
		})
		.await;
	}

	/// A picture waits for the sound stamped with it, which plays the jitter
	/// estimate after it arrives, rather than showing on its own delay.
	#[tokio::test(start_paused = true)]
	async fn a_picture_waits_for_its_sound() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, origin) = context(dir.path());
			let mut live = live(&origin, true).await;
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection());
			start(&sources, drivers).await;
			let feed = sources.get("guest").unwrap();
			let clock = feed.clock.clone().unwrap();

			until_time(kio::clock::now()).await;
			let arrived = kio::clock::now();
			let silence = vec![0u8; 960 * 2 * size_of::<f32>()];
			let stamp = moq_net::Timestamp::from_millis(1_000).unwrap();
			let audio = live.audio.as_mut().unwrap();
			audio.write(&moq_audio::Frame::new(silence.into(), stamp)).unwrap();
			picture(&mut live.video, &mut live.encoder, 1_000);
			let target = until(|| clock.target()).await;
			assert!(target + crate::playout::WRITE_LEAD > VIDEO_DELAY, "{target:?}");
			let due = arrived + target + crate::playout::WRITE_LEAD;
			until(|| (clock.due(stamp) == Some(due)).then_some(())).await;
			until_time(due - Duration::from_millis(1)).await;
			assert!(feed.next_picture().is_none(), "shown before its sound");
			until_time(due).await;
			assert_eq!(shown(feed).await, Duration::from(stamp));
			assert_eq!(feed.delay(), Some(target));
		})
		.await;
	}

	/// Dropping a feed stops its driver, which is what releases a device.
	#[tokio::test]
	async fn a_dropped_feed_stops_its_driver() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let mut drivers = sources.sync(&collection());
			let driver = drivers.pop().unwrap();
			let running = tokio::task::spawn_local(driver.run());
			tokio::task::yield_now().await;
			assert!(!running.is_finished());

			sources.sync(&Collection::new());
			running.await.unwrap();
		})
		.await;
	}

	#[tokio::test]
	async fn a_driver_that_cannot_run_fails_its_feed() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			for driver in sources.sync(&collection()) {
				driver.fail("no thread");
			}
			for (name, feed) in sources.iter() {
				assert_eq!(feed.status(), Status::Failed("no thread".into()), "{name}");
			}
		})
		.await;
	}

	#[tokio::test]
	async fn an_image_loads_relative_to_the_scene_file() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let mut png = image::RgbaImage::new(3, 2);
			png.put_pixel(2, 1, image::Rgba([1, 2, 3, 4]));
			png.save(dir.path().join("logo.png")).unwrap();

			let mut collection = Collection::new();
			collection.sources.insert(
				"logo".into(),
				Source::Image {
					path: "logo.png".into(),
				},
			);
			collection.sources.insert(
				"missing".into(),
				Source::Image {
					path: "missing.png".into(),
				},
			);
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;

			let Some(Picture::Rgba { size, pixels }) = sources.get("logo").unwrap().next_picture() else {
				panic!("an image shows a picture");
			};
			assert_eq!(size, moq_video::Size::new(3, 2));
			assert_eq!(pixels[pixels.len() - 4..], [1, 2, 3, 4]);

			let Status::Failed(reason) = sources.get("missing").unwrap().status() else {
				panic!("a missing image fails");
			};
			assert!(reason.contains("missing.png"), "{reason}");
		})
		.await;
	}

	/// Without a helper to run it in, a browser source fails rather than
	/// staying blank, until a restart starts a new page.
	#[tokio::test]
	async fn a_page_without_a_helper_says_so() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let mut collection = Collection::new();
			collection.sources.insert(
				"chat".into(),
				Source::Browser {
					url: "https://example.com".into(),
					width: 400,
					height: 800,
				},
			);
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			let chat = sources.get("chat").unwrap();
			let Status::Failed(reason) = chat.status() else {
				panic!("a page without a helper fails");
			};
			assert!(reason.contains("web pages") || reason.contains("Linux"), "{reason}");
			assert!(chat.page().is_some_and(|page| page.paint().is_none()));
			assert!(chat.strip().is_some(), "a page feeds the bus");

			let failed = chat.page().unwrap().clone();
			sources.restart("chat");
			assert_eq!(sources.sync(&collection).len(), 1, "a restart starts it again");
			assert!(!sources.get("chat").unwrap().page().unwrap().same(&failed));
		})
		.await;
	}

	/// An overlay goes live for the compositor to paint, and one outside what
	/// overlays draw fails the source, naming what.
	#[tokio::test]
	async fn an_overlay_fails_on_what_it_cannot_draw() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let overlay = |html: &str| Source::Overlay {
				html: html.into(),
				width: 640,
				height: 360,
				restart: true,
			};
			let mut collection = Collection::new();
			collection.sources.insert("title".into(), overlay("<h1>Live</h1>"));
			collection.sources.insert(
				"shadow".into(),
				overlay(r#"<h1 style="text-shadow: 0 0 4px">Live</h1>"#),
			);
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;

			let title = sources.get("title").unwrap();
			assert_eq!(title.status(), Status::Live);
			assert!(matches!(title.next_picture(), Some(Picture::Overlay(_))));
			let Status::Failed(reason) = sources.get("shadow").unwrap().status() else {
				panic!("an overlay outside the subset fails");
			};
			assert!(reason.contains("`text-shadow`"), "{reason}");
		})
		.await;
	}

	#[cfg(not(feature = "capture"))]
	#[tokio::test]
	async fn a_build_without_capture_says_so() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let mut collection = Collection::new();
			collection.sources.insert("cam".into(), Source::Camera { id: None });
			collection.sources.insert("mic".into(), Source::Microphone { id: None });
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			for name in ["cam", "mic"] {
				let Status::Failed(reason) = sources.get(name).unwrap().status() else {
					panic!("{name} cannot capture");
				};
				assert!(reason.contains("capture"), "{reason}");
			}
		})
		.await;
	}

	/// A replay shows the span's pictures, and only those, at its speed: half
	/// speed spreads a second of them over two. Then it shows nothing.
	#[tokio::test(start_paused = true)]
	async fn a_replay_plays_its_span_at_its_speed() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let mut collection = Collection::new();
			collection.sources.insert("replay".into(), Source::Replay);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			let feed = sources.get("replay").unwrap();
			assert!(matches!(feed.next_picture(), Some(Picture::Rgba { pixels, .. }) if pixels == [0; 4]));

			let (broadcast, video, _live) = video(None).await;
			let target = Target {
				broadcast: broadcast.consume(),
				video: Some(video),
				audio: None,
				end: moq_net::Timestamp::from_micros(2_500_000).unwrap(),
			};
			let take = Take::new(&target, Duration::from_secs(1)).unwrap();
			let speed = 0.5;
			feed.replay(take, speed).unwrap();
			assert!(
				matches!(feed.next_picture(), Some(Picture::Rgba { .. })),
				"cleared as it is asked"
			);
			let begin = until(|| feed.run().begun).await;

			// From the start, not from the keyframe of its group at 1 s, each
			// picture `t / speed` into the replay, and none sooner. The clock
			// is held until the replay begins, then moves only to each
			// picture's moment, which the timer wheel rounds up to its
			// millisecond.
			let start = moq_net::Timestamp::from_micros(1_500_000).unwrap();
			for index in 45..75u64 {
				let timestamp = moq_net::Timestamp::from_micros(index * 1_000_000 / 30).unwrap();
				let due = begin + (Duration::from(timestamp) - Duration::from(start)).div_f32(speed);
				assert!(feed.next_picture().is_none(), "a picture before {timestamp:?} is due");
				until_time(due).await;
				let shown = until(|| match feed.next_picture() {
					Some(Picture::Frame(frame)) => Some(frame.timestamp),
					Some(_) => panic!("cleared before {timestamp:?}"),
					None => None,
				})
				.await;
				assert_eq!(shown, timestamp);
			}

			// Nothing more, and the replay ends once the span's second has
			// played over two.
			assert!(
				feed.next_picture().is_none(),
				"the picture at the end shows for no time"
			);
			let end = begin + Duration::from_secs(2);
			until_time(end - Duration::from_millis(1)).await;
			assert!(feed.next_picture().is_none(), "cleared before the end");
			until_time(end).await;
			until(|| matches!(feed.next_picture(), Some(Picture::Rgba { .. })).then_some(())).await;
			assert_eq!(feed.run().ended, Some(end));
			assert_eq!(feed.status(), Status::Live, "ready for the next replay");
		})
		.await;
	}

	/// A new replay cuts off the one playing, whose picture does not linger
	/// until the new one's first is due.
	#[tokio::test(start_paused = true)]
	async fn a_new_replay_clears_the_one_playing() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let mut collection = Collection::new();
			collection.sources.insert("replay".into(), Source::Replay);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			let feed = sources.get("replay").unwrap();

			let (broadcast, video, _live) = video(None).await;
			let take = |end| {
				let target = Target {
					broadcast: broadcast.consume(),
					video: Some(video.clone()),
					audio: None,
					end: moq_net::Timestamp::from_micros(end).unwrap(),
				};
				Take::new(&target, Duration::from_secs(1)).unwrap()
			};
			// Its first picture, at 1.5 s, is due as it begins.
			feed.replay(take(2_500_000), 1.0).unwrap();
			until_time(until(|| feed.run().begun).await).await;
			until(|| matches!(feed.next_picture(), Some(Picture::Frame(_))).then_some(())).await;
			// This one's first picture, at the same 1.5 s, is due 20 ms in. The
			// one playing is cleared as it is asked, before the driver hears
			// of it, so nothing it shows meanwhile is drawn.
			feed.replay(take(2_480_000), 1.0).unwrap();
			let shown = feed.next_picture().expect("cleared at once");
			assert!(matches!(shown, Picture::Rgba { pixels, .. } if pixels == [0; 4]));
		})
		.await;
	}

	/// A replay that starts in a gap, just before the broadcast resumed,
	/// decodes the first picture after it before it begins, so that picture
	/// shows on time.
	#[tokio::test(start_paused = true)]
	async fn a_replay_from_a_gap_shows_its_first_picture_on_time() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let mut collection = Collection::new();
			collection.sources.insert("replay".into(), Source::Replay);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			let feed = sources.get("replay").unwrap();

			// Paused through its second second, resuming with a picture at 2 s,
			// a millisecond after the span starts.
			let (broadcast, video, _live) = video(Some(1)).await;
			let target = Target {
				broadcast: broadcast.consume(),
				video: Some(video),
				audio: None,
				end: moq_net::Timestamp::from_micros(2_500_000).unwrap(),
			};
			let take = Take::new(&target, Duration::from_micros(501_000)).unwrap();
			feed.replay(take, 1.0).unwrap();
			let begun = until(|| feed.run().begun).await;
			// Past its moment, without holding the clock for a decode.
			until_time(begun + Duration::from_millis(2)).await;
			let Some(Picture::Frame(frame)) = feed.next_picture() else {
				panic!("the first picture is not up on time");
			};
			assert_eq!(frame.timestamp, moq_net::Timestamp::from_micros(2_000_000).unwrap());
		})
		.await;
	}
}

#[cfg(test)]
mod media_tests {
	use super::test::{context, listen, local, start, until, until_time};
	use super::*;
	use crate::media::tests::{CLIP, SILENT, TONE};

	/// A collection of media sources, each named for its file in `dir`, which
	/// is written with its bytes, and whether it loops and restarts.
	fn media(dir: &Path, files: &[(&str, &[u8], bool, bool)]) -> Collection {
		let mut collection = Collection::new();
		for (name, bytes, looping, restart) in files {
			std::fs::write(dir.join(name), bytes).unwrap();
			let source = Source::Media {
				path: name.into(),
				looping: *looping,
				restart: *restart,
			};
			collection.sources.insert(name.to_string(), source);
			collection.audio_scenes[0].sounds.insert(name.to_string());
		}
		collection
	}

	/// The next picture `feed` shows, passing over the clear a restart puts up,
	/// and when it showed. The clock moves only to a play's beginning, which
	/// comes the write lead after its first picture decodes.
	async fn shown(feed: &Feed) -> (moq_net::Timestamp, Instant) {
		loop {
			let next = until(|| match feed.next_picture() {
				Some(Picture::Frame(frame)) => Some(Ok(frame.timestamp)),
				Some(Picture::Rgba { pixels, .. }) if pixels == [0; 4] => None,
				Some(_) => panic!("not a decoded picture"),
				None => feed.run().begun.filter(|begun| *begun > kio::clock::now()).map(Err),
			})
			.await;
			match next {
				Ok(timestamp) => return (timestamp, kio::clock::now()),
				Err(begun) => until_time(begun).await,
			}
		}
	}

	/// Wait until `name` reads as `status`, without moving the clock.
	async fn until_status(sources: &Sources, name: &str, status: Status) {
		until(|| (sources.get(name).unwrap().status() == status).then_some(())).await;
	}

	/// A file shows each picture at its moment, the first at once. Past its
	/// end it holds the last picture, reads as ended, and plays again from
	/// its start when asked.
	#[tokio::test(start_paused = true)]
	async fn a_media_file_plays_once_and_holds_its_end() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let collection = media(dir.path(), &[("clip.mkv", CLIP, false, false)]);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			let feed = sources.get("clip.mkv").unwrap();

			let (first, begin) = shown(feed).await;
			assert_eq!(feed.status(), Status::Live);
			for index in 1..10u64 {
				let into = Duration::from_millis(index * 100);
				// Within the timer wheel's millisecond of when the test saw the first.
				until_time(begin + into - Duration::from_millis(2)).await;
				assert!(feed.next_picture().is_none(), "picture {index} before its moment");
				until_time(begin + into).await;
				let (timestamp, _) = shown(feed).await;
				assert_eq!(Duration::from(timestamp) - Duration::from(first), into);
			}

			// The last picture lasts as long as the one before it, and the
			// sound runs a little past it.
			until_time(begin + Duration::from_millis(990)).await;
			assert_eq!(feed.status(), Status::Live);
			until_time(begin + Duration::from_millis(1_100)).await;
			until_status(&sources, "clip.mkv", Status::Ended).await;
			assert!(feed.next_picture().is_none(), "the last picture holds");

			feed.play().unwrap();
			let cleared = feed.next_picture();
			assert!(
				matches!(cleared, Some(Picture::Rgba { pixels, .. }) if pixels == [0; 4]),
				"the held picture is cleared at once"
			);
			let (again, _) = shown(feed).await;
			assert_eq!(again, first, "from the start");
			assert_eq!(feed.status(), Status::Live);
		})
		.await;
	}

	/// The file a stinger plays opens blank and silent, waiting for its first
	/// play, and each play reports when it began and ended, never a play it
	/// cut off.
	#[tokio::test(start_paused = true)]
	async fn a_stinger_clip_waits_for_its_play() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let mut collection = media(dir.path(), &[("clip.mkv", CLIP, false, false)]);
			collection.transition = crate::Transition::Stinger {
				source: "clip.mkv".into(),
				cut: Duration::from_millis(500),
			};
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			let feed = sources.get("clip.mkv").unwrap();
			assert_eq!(feed.status(), Status::Ended);
			until_time(kio::clock::now() + Duration::from_millis(1_100)).await;
			assert!(feed.next_picture().is_none(), "nothing plays until asked");
			assert_eq!(feed.run(), Run::default());

			feed.play().unwrap();
			assert_eq!(feed.run().play, 1);
			let (first, shown_at) = shown(feed).await;
			let begun = feed.run().begun.expect("begun once its first picture is due");
			assert!(begun <= shown_at, "{begun:?} after {shown_at:?}");
			assert_eq!(feed.run().ended, None);

			// Cut off halfway, it never ends; the new play starts over.
			until_time(begun + Duration::from_millis(500)).await;
			feed.play().unwrap();
			let forgotten = Run {
				play: 2,
				..Run::default()
			};
			assert_eq!(feed.run(), forgotten, "the play before is forgotten");
			let (again, _) = shown(feed).await;
			assert_eq!(again, first);
			let begun = feed.run().begun.unwrap();

			until_time(begun + Duration::from_millis(1_100)).await;
			until_status(&sources, "clip.mkv", Status::Ended).await;
			let ended = feed.run().ended.expect("ended at its end");
			assert!(
				ended >= begun + Duration::from_secs(1) && ended <= kio::clock::now(),
				"{:?} after it began",
				ended - begun
			);
		})
		.await;
	}

	/// A stinger's file asked to play while it still opens plays once open.
	#[tokio::test(start_paused = true)]
	async fn a_stinger_clip_asked_while_opening_plays() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let mut collection = media(dir.path(), &[("clip.mkv", CLIP, false, false)]);
			collection.transition = crate::Transition::Stinger {
				source: "clip.mkv".into(),
				cut: Duration::ZERO,
			};
			let drivers = sources.sync(&collection);
			let feed = sources.get("clip.mkv").unwrap();
			assert_eq!(feed.status(), Status::Starting);
			feed.play().unwrap();
			start(&sources, drivers).await;
			shown(feed).await;
			assert_eq!(feed.run().play, 1);
			assert!(feed.run().begun.is_some());
		})
		.await;
	}

	/// A playing file stopped goes blank and silent at once, sound it had
	/// queued included, ending its play there, and plays from its start
	/// again when asked. Only a media source stops.
	#[tokio::test(start_paused = true)]
	async fn a_stopped_media_file_goes_blank_and_silent() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (mut context, _origin) = context(dir.path());
			let (bus, driver) = Bus::new(Default::default()).unwrap();
			let mut at = kio::clock::now();
			let (mut mixer, mixed) = Mixer::new(driver, at);
			context.bus = bus;
			context.mixed = mixed;
			let mut sources = Sources::new(context);
			let mut collection = media(dir.path(), &[("clip.mkv", CLIP, false, false)]);
			let red = Source::Color {
				color: crate::Rgba([255, 0, 0, 255]),
			};
			collection.sources.insert("red".into(), red);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			let feed = sources.get("clip.mkv").unwrap();
			let program = sources.mix(crate::AudioScene::DEFAULT).unwrap().clone();

			let (first, begun) = shown(feed).await;
			let heard = listen(&mut mixer, &program, &mut at, begun + Duration::from_millis(300)).await;
			assert!(heard > 0.0, "the clip sounds before it stops");

			feed.stop().unwrap();
			let stopped = kio::clock::now();
			let blank = feed.next_picture();
			assert!(
				matches!(blank, Some(Picture::Rgba { pixels, .. }) if pixels == [0; 4]),
				"blank at once"
			);
			assert_eq!(feed.run().ended, Some(stopped));
			until_status(&sources, "clip.mkv", Status::Ended).await;
			let heard = listen(&mut mixer, &program, &mut at, stopped + Duration::from_millis(500)).await;
			assert_eq!(heard, 0.0, "nothing it queued plays");
			assert!(
				!matches!(feed.next_picture(), Some(Picture::Frame(_))),
				"no picture after the stop"
			);
			assert_eq!(feed.run().ended, Some(stopped), "the play stays ended there");

			feed.play().unwrap();
			let (again, _) = shown(feed).await;
			assert_eq!(again, first, "from the start");
			assert_eq!(feed.status(), Status::Live);
			let red = sources.get("red").unwrap();
			assert!(matches!(red.stop(), Err(Error::Unsupported(_))));
		})
		.await;
	}

	/// A looping file starts over at its end, never reading as ended.
	#[tokio::test(start_paused = true)]
	async fn a_looping_media_file_starts_over() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let collection = media(dir.path(), &[("clip.mkv", CLIP, true, false)]);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			let feed = sources.get("clip.mkv").unwrap();

			let (first, begin) = shown(feed).await;
			until_time(begin + Duration::from_millis(950)).await;
			let (last, _) = shown(feed).await;
			assert_eq!(Duration::from(last) - Duration::from(first), Duration::from_millis(900));
			until_time(begin + Duration::from_millis(998)).await;
			assert!(feed.next_picture().is_none(), "the start again before the end");
			until_time(begin + Duration::from_millis(1_100)).await;
			let (again, _) = shown(feed).await;
			assert_eq!(again, first);
			assert_eq!(feed.status(), Status::Live);
		})
		.await;
	}

	/// A file set to restart plays from its start as it goes on program, not
	/// again while it stays on, and one not set to restart never does.
	#[tokio::test(start_paused = true)]
	async fn a_media_file_restarts_going_on_program() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let collection = media(
				dir.path(),
				&[("intro.mkv", CLIP, false, true), ("still.mkv", CLIP, false, false)],
			);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			let (first, begin) = shown(sources.get("intro.mkv").unwrap()).await;
			// Each clip starts once its first picture decodes, on a thread
			// tokio cannot see. Waiting for both while the clock holds starts
			// them together, so both end before the clock passes 1.1 s.
			shown(sources.get("still.mkv").unwrap()).await;
			until_time(begin + Duration::from_millis(1_100)).await;
			until_status(&sources, "intro.mkv", Status::Ended).await;
			until_status(&sources, "still.mkv", Status::Ended).await;
			for name in ["intro.mkv", "still.mkv"] {
				sources.get(name).unwrap().take();
			}

			let both: BTreeSet<String> = ["intro.mkv".into(), "still.mkv".into()].into();
			sources.set_on_program(&both);
			let (again, begin) = shown(sources.get("intro.mkv").unwrap()).await;
			assert_eq!(again, first);
			assert_eq!(sources.get("still.mkv").unwrap().status(), Status::Ended);

			until_time(begin + Duration::from_millis(1_100)).await;
			until_status(&sources, "intro.mkv", Status::Ended).await;
			sources.set_on_program(&both);
			assert_eq!(
				sources.get("intro.mkv").unwrap().status(),
				Status::Ended,
				"still on program"
			);

			sources.set_on_program(&BTreeSet::new());
			sources.set_on_program(&both);
			until_status(&sources, "intro.mkv", Status::Live).await;
			assert!(sources.get("still.mkv").unwrap().take().is_none());
		})
		.await;
	}

	/// One bus block: how often the bus mixes, and so how finely a mix places
	/// a sound in time.
	const BLOCK: Duration = Duration::from_millis(10);

	/// The bus mixed along with a paused clock a block at a time, each once
	/// the clock has passed it, as the program's audio clock mixes it, keeping
	/// the first channel of what one mix heard.
	struct Ear {
		mixer: Mixer,
		reader: moq_audio::bus::Reader,
		/// When the bus starts, so a frame `n` in sounds `n` samples after it.
		origin: Instant,
		at: Instant,
		heard: Vec<f32>,
	}

	impl Ear {
		/// Mix along with the clock from `sources`' bus start.
		fn new(mixer: Mixer, sources: &Sources, mix: &Mix) -> Self {
			let origin = sources.context().mixed.start();
			Self {
				mixer,
				reader: mix.reader(),
				origin,
				at: origin,
				heard: Vec::new(),
			}
		}

		/// Move the clock a block on, and mix that block.
		async fn step(&mut self) {
			self.at += BLOCK;
			until_time(self.at).await;
			self.mix();
		}

		fn mix(&mut self) {
			self.mixer.advance(Self::frames(BLOCK));
			while let std::task::Poll::Ready(Some(block)) = self.reader.poll_read(&kio::Waiter::noop()) {
				let channels = block.data.len() / block.frames();
				assert_eq!(block.position as usize, self.heard.len(), "a block skipped");
				self.heard.extend(block.data.iter().step_by(channels));
			}
		}

		/// The frames the bus mixes over `duration`.
		fn frames(duration: Duration) -> usize {
			let rate = moq_audio::bus::Config::default().sample_rate;
			(duration.as_nanos() * u128::from(rate) / 1_000_000_000) as usize
		}

		/// When the mix first heard anything.
		fn onset(&self) -> Option<Instant> {
			let index = self.heard.iter().position(|sample| sample.abs() > 0.01)?;
			let rate = moq_audio::bus::Config::default().sample_rate;
			Some(self.origin + Duration::from_secs_f64(index as f64 / f64::from(rate)))
		}

		/// The loudest the mix heard over `duration` from `from`.
		fn peak(&self, from: Instant, duration: Duration) -> f32 {
			let start = Self::frames(from - self.origin);
			let heard = &self.heard[start..start + Self::frames(duration)];
			heard.iter().fold(0.0, |peak, sample| peak.max(sample.abs()))
		}
	}

	/// A file's first picture shows as its first sound is heard, within a bus
	/// block, rather than ahead of it.
	#[tokio::test(start_paused = true)]
	async fn a_media_files_sound_plays_with_its_pictures() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (mut context, _origin) = context(dir.path());
			let (bus, driver) = Bus::new(Default::default()).unwrap();
			let (mixer, mixed) = Mixer::new(driver, kio::clock::now());
			context.bus = bus;
			context.mixed = mixed;
			let mut sources = Sources::new(context);
			let collection = media(dir.path(), &[("clip.mkv", CLIP, false, false)]);
			let drivers = sources.sync(&collection);
			let program = sources.mix(crate::AudioScene::DEFAULT).unwrap().clone();
			let mut ear = Ear::new(mixer, &sources, &program);
			start(&sources, drivers).await;
			in_step(&mut ear, sources.get("clip.mkv").unwrap()).await;
		})
		.await;
	}

	/// A replay's first picture shows as its first sound is heard, within a
	/// bus block, rather than ahead of it.
	#[tokio::test(start_paused = true)]
	async fn a_replays_sound_plays_with_its_pictures() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (mut context, _origin) = context(dir.path());
			let (bus, driver) = Bus::new(Default::default()).unwrap();
			let (mixer, mixed) = Mixer::new(driver, kio::clock::now());
			context.bus = bus;
			context.mixed = mixed;
			let mut sources = Sources::new(context);
			let mut collection = Collection::new();
			collection.sources.insert("replay".into(), Source::Replay);
			collection.audio_scenes[0].sounds.insert("replay".into());
			let drivers = sources.sync(&collection);
			let program = sources.mix(crate::AudioScene::DEFAULT).unwrap().clone();
			let mut ear = Ear::new(mixer, &sources, &program);
			start(&sources, drivers).await;

			// The clip's whole second, pictures and sound from its start.
			std::fs::write(dir.path().join("clip.mkv"), CLIP).unwrap();
			let clip = Clip::open(&dir.path().join("clip.mkv")).unwrap();
			let target = Target {
				broadcast: clip.consume(),
				video: clip.video.clone(),
				audio: clip.audio.clone(),
				end: moq_net::Timestamp::from_secs(1).unwrap(),
			};
			let take = Take::new(&target, Duration::from_secs(1)).unwrap();
			let feed = sources.get("replay").unwrap();
			feed.replay(take, 1.0).unwrap();
			in_step(&mut ear, feed).await;
		})
		.await;
	}

	/// A replay that starts part way through a sound plays the rest of it as
	/// it begins, rather than silence until the next.
	#[tokio::test(start_paused = true)]
	async fn a_replay_plays_the_sound_across_its_start() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (mut context, _origin) = context(dir.path());
			let (bus, driver) = Bus::new(Default::default()).unwrap();
			let (mixer, mixed) = Mixer::new(driver, kio::clock::now());
			context.bus = bus;
			context.mixed = mixed;
			let mut sources = Sources::new(context);
			let mut collection = Collection::new();
			collection.sources.insert("replay".into(), Source::Replay);
			collection.audio_scenes[0].sounds.insert("replay".into());
			let drivers = sources.sync(&collection);
			let program = sources.mix(crate::AudioScene::DEFAULT).unwrap().clone();
			let mut ear = Ear::new(mixer, &sources, &program);
			start(&sources, drivers).await;

			// The clip's sound alone, from 5 ms in: inside its first 20 ms frame.
			std::fs::write(dir.path().join("clip.mkv"), CLIP).unwrap();
			let clip = Clip::open(&dir.path().join("clip.mkv")).unwrap();
			let target = Target {
				broadcast: clip.consume(),
				video: None,
				audio: clip.audio.clone(),
				end: moq_net::Timestamp::from_millis(505).unwrap(),
			};
			let take = Take::new(&target, Duration::from_millis(500)).unwrap();
			let feed = sources.get("replay").unwrap();
			feed.replay(take, 1.0).unwrap();
			// Held until it begins, so the first sound is written as the bus
			// last mixed, and is heard just as the replay begins.
			let begun = until(|| feed.run().begun).await;
			while ear.at < begun + Duration::from_millis(50) {
				ear.step().await;
			}
			let onset = ear.onset().expect("it sounds");
			assert!(
				onset < begun + Duration::from_millis(2),
				"heard as it begins: {:?} after",
				onset.saturating_duration_since(begun)
			);
		})
		.await;
	}

	/// Mix along with `feed`'s latest play until its first picture shows and
	/// its first sound is heard, and check both do as it begins: the picture
	/// at once, and the sound within a bus block.
	async fn in_step(ear: &mut Ear, feed: &Feed) {
		while !matches!(feed.next_picture(), Some(Picture::Frame(_))) {
			ear.step().await;
		}
		// Seen at the step after it showed, which may be the block after,
		// when the step woke before the picture's timer at the same moment.
		let shown = ear.at;
		let begun = feed.run().begun.unwrap();
		assert!(
			begun <= shown && shown - begun <= BLOCK,
			"the first picture shows when it begins: {:?} after",
			shown - begun
		);
		while ear.at < begun + Duration::from_millis(100) {
			ear.step().await;
		}
		let onset = ear.onset().expect("it sounds");
		let apart = onset
			.saturating_duration_since(begun)
			.max(begun.saturating_duration_since(onset));
		assert!(
			apart < BLOCK,
			"its first sound is heard with its first picture: {onset:?} and {begun:?}"
		);
	}

	/// A looping file's sound runs on from its end into its start again, as
	/// loud as the first time and as its pass begins, each pass heard from
	/// when it begins.
	#[tokio::test(start_paused = true)]
	async fn a_looping_files_sound_runs_on() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (mut context, _origin) = context(dir.path());
			let (bus, driver) = Bus::new(Default::default()).unwrap();
			let (mixer, mixed) = Mixer::new(driver, kio::clock::now());
			context.bus = bus;
			context.mixed = mixed;
			let mut sources = Sources::new(context);
			let collection = media(dir.path(), &[("tone.webm", TONE, true, false)]);
			let drivers = sources.sync(&collection);
			let program = sources.mix(crate::AudioScene::DEFAULT).unwrap().clone();
			let mut ear = Ear::new(mixer, &sources, &program);
			start(&sources, drivers).await;
			let feed = sources.get("tone.webm").unwrap();

			let ended = loop {
				if let Some(ended) = feed.run().ended {
					break ended;
				}
				ear.step().await;
			};
			let begun = feed.run().begun.unwrap();
			while ear.at < ended + Duration::from_millis(100) {
				ear.step().await;
			}
			// Past a block that either may be heard early by, and the start of
			// the tone the codec fades in.
			let into = Duration::from_millis(15);
			let first = ear.peak(begun + into, Duration::from_millis(30));
			let again = ear.peak(ended + into, Duration::from_millis(30));
			assert!(first > 0.05, "the tone sounds: {first}");
			assert!(
				again > first / 2.0,
				"the loop starts again as loud: {again} after {first}"
			);
		})
		.await;
	}

	/// A file of sound alone shows nothing, and goes silent at its end. One of
	/// pictures alone has no sound for a mix to play.
	#[tokio::test(start_paused = true)]
	async fn an_audio_file_ends_silent() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let files = [("tone.webm", TONE, false, false), ("silent.mkv", SILENT, false, false)];
			let collection = media(dir.path(), &files);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			let program = crate::AudioScene::DEFAULT;
			assert!(sources.plays(program, "tone.webm"));
			assert!(!sources.plays(program, "silent.mkv"));
			assert!(sources.get("silent.mkv").unwrap().strip().is_none());
			assert_eq!(sources.get("tone.webm").unwrap().status(), Status::Live);
			until_time(kio::clock::now() + Duration::from_millis(600)).await;
			until_status(&sources, "tone.webm", Status::Ended).await;
			assert!(sources.get("tone.webm").unwrap().take().is_none());
		})
		.await;
	}

	/// A file the studio cannot read fails its source, saying why, and only a
	/// media source plays from its start.
	#[tokio::test]
	async fn a_media_file_it_cannot_read_fails() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let mut collection = media(dir.path(), &[("notes.txt", b"not a video", false, false)]);
			let red = Source::Color {
				color: crate::Rgba([255, 0, 0, 255]),
			};
			collection.sources.insert("red".into(), red);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;

			let feed = sources.get("notes.txt").unwrap();
			let reason = until(|| match feed.status() {
				Status::Failed(reason) => Some(reason),
				_ => None,
			})
			.await;
			assert!(
				reason.contains("notes.txt") && reason.contains("not a container"),
				"{reason}"
			);
			assert!(matches!(feed.play(), Err(Error::Stopped(_))));
			let red = sources.get("red").unwrap();
			assert!(matches!(red.play(), Err(Error::Unsupported(_))));
		})
		.await;
	}
}
