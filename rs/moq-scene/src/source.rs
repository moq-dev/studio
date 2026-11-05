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
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
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
use crate::playout::{MAX_DELAY, Place, Speaker, VIDEO_DELAY, Writer};
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
	/// An overlay whose HTML passed [`crate::overlay::check`], which the
	/// compositor lays out and paints itself, on the program clock.
	Overlay,
}

/// The one picture a source holds for the compositor. A newer picture replaces
/// an undrawn one, so a slow canvas skips pictures instead of queuing them.
pub(crate) type Mailbox = Arc<Mutex<Option<Picture>>>;

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

/// A take for a replay source to play, and how fast.
struct Play {
	take: Take,
	speed: f32,
}

/// A replay source's next replay, until its driver takes it, so a replay that
/// ended holds its take no longer.
type Next = Arc<Mutex<Option<Play>>>;

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
}

/// What every source shares.
#[derive(Clone)]
pub struct Context {
	/// Where microphones, system audio, and broadcasts feed their sound, and
	/// where the collection's mixes are made.
	pub bus: Bus,
	/// Where broadcast sources are announced: the studio's relay connection.
	pub origin: moq_net::origin::Consumer,
	/// What a relative image or LUT path is relative to: the scene file's directory.
	pub dir: PathBuf,
	/// What cameras, screens, and broadcasts deliver video as:
	/// `moq_video::render::output` of the compositor's device.
	pub output: moq_video::Output,
	/// What browser sources run in; without it they fail as unsupported.
	pub browser: Option<crate::browser::Helper>,
	/// Where local sources publish their own tracks, or `None` to publish nothing.
	pub publish: Option<Publish>,
}

/// The running sources of a collection, and its mixes.
pub struct Sources {
	context: Context,
	feeds: BTreeMap<String, Feed>,
	mixes: BTreeMap<String, Mix>,
}

impl Sources {
	/// No sources or mixes yet; [`sync`](Self::sync) starts them.
	pub fn new(context: Context) -> Self {
		Self {
			context,
			feeds: BTreeMap::new(),
			mixes: BTreeMap::new(),
		}
	}

	/// What every source shares.
	pub fn context(&self) -> &Context {
		&self.context
	}

	/// Run exactly the sources and mixes `collection` has: start new and edited
	/// sources and stop removed and edited ones. A clone shares the feed of the
	/// source it clones rather than opening one. Returns a driver for each
	/// source started, which does nothing until run.
	///
	/// Also applies the collection's gain, mute, delay, and routing to every
	/// audio strip, and its frame rate to every web page, which never restarts
	/// a source. A removed or edited source leaves every mix at once, so its
	/// replacement never plays over it. Panics on a gain or delay
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
		// A mix ends once its last handle drops, which may be a reader's.
		self.mixes.retain(|name, _| collection.mixes.names.contains(name));
		for name in &collection.mixes.names {
			if !self.mixes.contains_key(name) {
				self.mixes.insert(name.clone(), self.context.bus.mix());
			}
		}

		let mut drivers = Vec::new();
		for (name, source) in collection.sources.iter().filter(|(_, source)| source.opens()) {
			if !self.feeds.contains_key(name) {
				let (feed, driver) = open(name, source, &self.context);
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
				strip.set_delay(audio.delay);
				for (mix_name, mix) in &self.mixes {
					let routed = !audio.exclude.contains(mix_name);
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

	/// The mix called `name`, as the collection last synced had it.
	pub fn mix(&self, name: &str) -> Option<&Mix> {
		self.mixes.get(name)
	}

	/// Whether the mix called `mix` plays the source called `name`: it has
	/// sound, is not muted, and is routed into that mix.
	pub fn plays(&self, mix: &str, name: &str) -> bool {
		let strip = self.feeds.get(name).and_then(Feed::strip);
		let mix = self.mixes.get(mix);
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
	/// A broadcast source's playout clock.
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
		let play = Play { take, speed };
		// Fails once the driver is gone, which leaves the reason in the status.
		plays.send(Arc::new(Mutex::new(Some(play)))).map_err(|_| {
			Error::Stopped(match self.status() {
				Status::Failed(reason) => reason,
				status => format!("{status:?}"),
			})
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
	/// own scenes and mixes.
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
			Source::Camera { .. } | Source::Broadcast { .. } => Stall::Picture,
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

	/// The source's gain, mute, delay, and meter in every mix, if it has sound.
	pub fn strip(&self) -> Option<&Strip> {
		self.strip.as_ref()
	}

	/// A browser source's page: how it paints, and interaction with it.
	pub fn page(&self) -> Option<&Page> {
		self.page.as_ref()
	}

	/// The newest picture not yet drawn.
	pub(crate) fn take(&self) -> Option<Picture> {
		self.picture.lock().unwrap().take()
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
	clock: Option<timeline::Producer>,
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
				crate::overlay::check(html)?;
				self.show(Picture::Overlay);
				std::future::pending().await
			}
			Source::Broadcast { path } => self.broadcast(path).await,
			Source::Browser { url, width, height } => self.browser(url, *width, *height).await,
			Source::Replay => {
				let plays = self.plays.clone().expect("a replay source has plays");
				self.replays(plays, listing).await
			}
			Source::Clone { .. } => unreachable!("a clone opens no feed"),
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
		*self.picture.lock().unwrap() = Some(picture);
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
			| Source::Broadcast { .. }
			| Source::Browser { .. }
			| Source::Overlay { .. }
			| Source::Clone { .. }
			| Source::Lut { .. }
			| Source::Replay => unreachable!("not a device source"),
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
	#[cfg(feature = "capture")]
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
		let sound = self.context.bus.channel(input, strip)?;
		crate::browser::run(helper, open, page, &self.picture, Some(sound), &self.status).await
	}

	#[cfg(not(target_os = "linux"))]
	async fn browser(&self, _: &str, _: u32, _: u32) -> Result<(), Error> {
		Err(Error::Unsupported("browser sources run on Linux only for now".into()))
	}

	/// Follow the broadcast at `path` for as long as the feed lives, playing
	/// each instance of it that comes online.
	async fn broadcast(&self, path: &str) -> Result<(), Error> {
		let mut instances = moq_mux::follow::Instances::new(&self.context.origin, path, None)?;
		set(&self.status, Status::Offline);

		let mut clock = self.clock.clone().expect("a broadcast source has a clock");
		let mut playing: Option<Playing<'_>> = None;
		loop {
			tokio::select! {
				event = instances.next() => match event {
					Ok(moq_mux::follow::Event::Start(broadcast)) => {
						set(&self.status, Status::Live);
						*self.played.lock().unwrap() = Played {
							broadcast: Some(broadcast.clone()),
							..Played::default()
						};
						// Each instance's timestamps are its own.
						clock = clock.reset(VIDEO_DELAY);
						playing = Some(Box::pin(self.play(broadcast, path, clock.clone())));
					}
					Ok(moq_mux::follow::Event::End) => {
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
		let bus = self.context.bus.config().clone();
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

		let mut input = moq_audio::encode::Input::new(bus.sample_rate, bus.layout);
		input.format = moq_audio::Format::F32;
		let channel = self.context.bus.channel(input, strip)?;
		let mut writer = Writer::new(channel, bus.sample_rate, bus.layout.channels());
		let speaker = RefCell::new(Speaker::new(clock.timeline.clone(), &clock.changed));
		let stride = bus.layout.channels() as usize * size_of::<f32>();
		let rate = f64::from(bus.sample_rate);

		// Frames wait here for their moment, bounded by the target the speaker
		// holds, while every arrival is read at once: the jitter estimate times
		// each one as `read` hands it over.
		let queue = RefCell::new(VecDeque::new());
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
					queue.borrow_mut().push_back((sounds, frame));
					queued.notify_one();
				}
			}
			received.set(true);
			queued.notify_one();
			Ok::<_, Error>(())
		};
		// Writes each frame its bus latency ahead, and tells the clock about
		// the frames after a skip once they sound.
		let write = async {
			loop {
				let now = kio::clock::now();
				speaker.borrow_mut().commit(now);
				let write = queue
					.borrow()
					.front()
					.map(|(sounds, _)| *sounds - crate::playout::BUS_LATENCY);
				if write.is_some_and(|at| at <= now) {
					let (sounds, frame) = queue.borrow_mut().pop_front().expect("peeked");
					writer.write(sounds, &frame.data, now)?;
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
		*self.picture.lock().unwrap() = Some(clear());
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
					// A replay cut off must not leave its last picture up until
					// the new one's first. Between replays the picture is clear
					// already, and clearing it again would show before the new
					// one's first picture even when that is due at once.
					if playing.is_some() {
						*self.picture.lock().unwrap() = Some(clear());
					}
					playing = play.map(|play| Box::pin(self.replay(play, listing)) as Playing<'_>);
				}
				result = async { playing.as_mut().expect("guarded").await }, if playing.is_some() => {
					playing = None;
					*self.picture.lock().unwrap() = Some(clear());
					set(&self.status, match result {
						Ok(()) => Status::Live,
						Err(err) => Status::Failed(err.to_string()),
					});
				}
			}
		}
	}

	/// Play one take at its speed, each picture and sound when its moment
	/// comes: one `t` into the span plays `t / speed` after the replay starts.
	/// Ends once the whole span has played.
	///
	/// With `listing`, the pictures are also encoded onto the source's own
	/// track as they show, while it is used.
	async fn replay(&self, play: Play, listing: Option<&Listing<'_>>) -> Result<(), Error> {
		set(&self.status, Status::Live);
		let take = &play.take;
		let begin = kio::clock::now();
		let at = |timestamp: moq_net::Timestamp| {
			let into = Duration::from(timestamp).saturating_sub(Duration::from(take.start));
			begin + into.div_f32(play.speed)
		};
		let broadcast = take.consume();

		let video = async {
			let Some((name, config)) = &take.video else {
				return Ok(());
			};
			let mut options = moq_video::decode::Options::new();
			options.decoder.output = self.context.output;
			options.delay = REPLAY;
			let mut consumer = moq_video::decode::Consumer::new(&broadcast, config, name, options).await?;
			// Opened at the first picture, which says its size.
			let mut track: Option<crate::publish::Video> = None;
			let mut opened = false;
			while let Some(read) = consumer.read().await? {
				match read {
					// The group in progress at the start decodes from its
					// keyframe, but shows only from the start on.
					moq_video::decode::Read::Frame(frame) if frame.timestamp < take.start => {}
					// One at the end would show for no time at all.
					moq_video::decode::Read::Frame(frame) if frame.timestamp >= take.end => break,
					moq_video::decode::Read::Frame(mut frame) => {
						kio::clock::sleep_until(at(frame.timestamp)).await;
						if let Some(listing) = listing.filter(|_| !std::mem::replace(&mut opened, true)) {
							let framerate = config
								.framerate
								.filter(|rate| rate.is_finite() && *rate >= 1.0)
								.and_then(|rate| moq_video::FrameRate::new(rate.round() as u32, 1).ok())
								.unwrap_or_else(|| moq_video::FrameRate::new(30, 1).expect("a valid frame rate"));
							let video =
								crate::publish::Video::new(listing, frame.size(), framerate, frame.surface.color());
							track = self.publish(video.await);
						}
						if let Some(video) = &mut track {
							// Stamped when it shows, on a timeline of the replay's own.
							frame.timestamp = moq_net::Timestamp::from_micros(begin.elapsed().as_micros() as u64)
								.map_err(moq_video::Error::from)?;
							if let Err(err) = video.encode(&mut frame) {
								tracing::warn!(source = %self.name, %err, "cannot encode the replay");
								self.fail_track(Status::Failed(err.to_string()));
								track.take().expect("encoding").abort(&err);
							}
						}
						self.show(Picture::Frame(frame));
					}
					// A stretch the broadcast was offline, paused, or lost
					// replays as the gap it was, on its own track too. Past
					// the end, the replay is over anyway.
					moq_video::decode::Read::Gap { timestamp, .. }
					| moq_video::decode::Read::End { end: timestamp } => {
						kio::clock::sleep_until(at(timestamp.clamp(take.start, take.end))).await;
						*self.picture.lock().unwrap() = Some(clear());
						if let Some(video) = &mut track
							&& let Err(err) = video.discontinuity()
						{
							tracing::warn!(source = %self.name, %err, "cannot mark a gap in the replay");
							self.fail_track(Status::Failed(err.to_string()));
							track.take().expect("encoding").abort(&err);
						}
					}
				}
			}
			Ok::<_, Error>(())
		};

		// Slowed down, sound would need stretching, so it plays only in real time.
		let audio = async {
			let (Some((name, config)), Some(strip), true) = (&take.audio, &self.strip, play.speed == 1.0) else {
				return Ok(());
			};
			let bus = self.context.bus.config().clone();
			let mut options = moq_audio::decode::Options::new();
			options.delay = REPLAY;
			options.output.format = moq_audio::Format::F32;
			options.output.sample_rate = Some(bus.sample_rate);
			options.output.layout = Some(bus.layout);
			let mut consumer = moq_audio::decode::Consumer::new(&broadcast, config, name.clone(), options).await?;
			let mut input = moq_audio::encode::Input::new(bus.sample_rate, bus.layout);
			input.format = moq_audio::Format::F32;
			let mut channel = self.context.bus.channel(input, strip.clone())?;
			while let Some(read) = consumer.read().await? {
				match read {
					moq_audio::decode::Read::Frame(frame) if frame.timestamp < take.start => {}
					moq_audio::decode::Read::Frame(frame) if frame.timestamp >= take.end => break,
					moq_audio::decode::Read::Frame(frame) => {
						kio::clock::sleep_until(at(frame.timestamp)).await;
						channel.write(&frame.data)?;
					}
					_ => {}
				}
			}
			// The bus plays what is written after a buffer's delay, and drops
			// what a closed channel has not played, so it stays open until the
			// replay ends.
			std::future::pending().await
		};

		// The span ends where it does even when its tracks have not ended yet.
		let played = async {
			tokio::try_join!(video, audio)?;
			std::future::pending().await
		};
		// Played first, so a picture due at the very end shows before it does.
		tokio::select! {
			biased;
			result = played => result,
			() = kio::clock::sleep_until(at(take.end)) => Ok(()),
		}
	}
}

/// How a replay reads its take: from the first group, where the take's tracks
/// declare they start, skipping nothing as late, since a take is history.
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
fn pick<'a, C: Reference + Clone + 'a>(
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

/// Start the feed for `source`, and the driver that fills it.
fn open(name: &str, source: &Source, context: &Context) -> (Feed, Driver) {
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
		.filter(|_| publishes(source))
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
		clock: clock.as_ref().map(timeline::Producer::consume),
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
		clock,
	};
	(feed, driver)
}

/// Whether `source` publishes its own track: a picture the studio captures or
/// loads itself, before the scene's filters, or the sound of a microphone or
/// system audio as captured, before its gain and mute.
///
/// A broadcast is already published by its own publisher, and a clone shows
/// its source's track, its filters carried by the layout rather than encoded
/// again. Web pages and overlays are seen only in the program, since H.264
/// would encode their transparency over black, and a LUT is looked up, never
/// shown.
fn publishes(source: &Source) -> bool {
	match source {
		Source::Camera { .. }
		| Source::Display { .. }
		| Source::Window { .. }
		| Source::Microphone { .. }
		| Source::SystemAudio
		| Source::Color { .. }
		| Source::Image { .. }
		| Source::Replay => true,
		Source::Broadcast { .. }
		| Source::Browser { .. }
		| Source::Overlay { .. }
		| Source::Clone { .. }
		| Source::Lut { .. } => false,
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
	let image = image::load_from_memory(&bytes)
		.map_err(|err| Error::Image(format!("{}: {err}", path.display())))?
		.into_rgba8();
	let picture = Picture::Rgba {
		size: moq_video::Size::new(image.width(), image.height()),
		pixels: image.into_raw(),
	};
	Ok((picture, bytes.into()))
}

/// Helpers for tests across the crate.
#[cfg(test)]
pub(crate) mod test {
	use super::*;

	/// A context with no relay or device behind it, so broadcasts stay offline
	/// while the returned origin lives. Call from a [`tokio::task::LocalSet`].
	pub(crate) fn context(dir: &Path) -> (Context, moq_net::origin::Producer) {
		let (origin, driver) = moq_net::origin::Producer::new(Default::default());
		tokio::task::spawn_local(moq_net::time::run(driver));
		let (bus, _) = Bus::new(Default::default()).unwrap();
		let context = Context {
			bus,
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
		let reserved = catalog.reserve().with_interval(config.gop.duration(config.framerate));
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
		collection
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
			let Some(Picture::Rgba { size, pixels }) = red.take() else {
				panic!("a color shows a picture");
			};
			assert_eq!(size, moq_video::Size::new(1, 1));
			assert_eq!(pixels, [255, 0, 0, 255]);
			assert!(red.take().is_none(), "taken once");

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
				let (feed, driver) = open("source", &source, &context);
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

	/// A mix plays a source with sound that is routed into it and not muted, so
	/// one left out of the program's mix is heard only in the others.
	#[tokio::test]
	async fn a_mix_plays_only_what_is_routed_into_it() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (context, _origin) = context(dir.path());
			let mut sources = Sources::new(context);
			let mut collection = collection();
			collection.mixes.names.push("Headphones".into());
			let program = crate::Mixes::DEFAULT;
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			assert!(sources.plays(program, "guest"));
			assert!(sources.plays("Headphones", "guest"));
			assert!(!sources.plays(program, "red"), "a color is silent");
			assert!(!sources.plays(program, "nobody"));
			assert!(!sources.plays("Nothing", "guest"));

			// A restart, such as a page's reload, leaves every mix at once, and
			// the source comes back in under its new strip.
			let before = sources.get("guest").unwrap().strip().unwrap().clone();
			sources.restart("guest");
			for mix in [program, "Headphones"] {
				assert!(!sources.mix(mix).unwrap().routes(&before), "{mix} keeps the old strip");
			}
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			assert!(sources.plays(program, "guest"));

			let left_out = Audio {
				exclude: [program.to_string()].into(),
				..Audio::default()
			};
			collection.audio.insert("guest".into(), left_out);
			assert!(sources.sync(&collection).is_empty());
			assert!(!sources.plays(program, "guest"), "left out of the program");
			assert!(sources.plays("Headphones", "guest"));

			let muted = Audio {
				muted: true,
				..Audio::default()
			};
			collection.audio.insert("guest".into(), muted);
			assert!(sources.sync(&collection).is_empty());
			assert!(!sources.plays(program, "guest"), "muted");
			assert!(!sources.plays("Headphones", "guest"), "muted everywhere");
		})
		.await;
	}

	/// Editing a source restarts it, removing one stops it, and an untouched one
	/// keeps running. Gain, delay, and routing changes reach the strip without
	/// restarting anything, and a source plays in every mix it is not left out
	/// of until it is removed.
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
			let program = sources.mix(crate::Mixes::DEFAULT).unwrap().clone();
			assert!(program.routes(&guest));

			collection.mixes.names.push("Stream".into());
			collection.audio.insert(
				"guest".into(),
				Audio {
					gain: 0.25,
					muted: true,
					exclude: ["Stream".to_string()].into(),
					delay: Duration::from_millis(120),
				},
			);
			assert!(sources.sync(&collection).is_empty());
			assert_eq!(guest.gain(), 0.25);
			assert!(guest.muted());
			assert_eq!(guest.delay(), Duration::from_millis(120));
			let stream = sources.mix("Stream").unwrap().clone();
			assert!(program.routes(&guest));
			assert!(!stream.routes(&guest), "left out of the new mix");

			collection.audio.get_mut("guest").unwrap().exclude.clear();
			assert!(sources.sync(&collection).is_empty());
			assert!(stream.routes(&guest), "put back in");

			collection.mixes.names.retain(|name| name != "Stream");
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
			let Some(Picture::Rgba { pixels, .. }) = sources.get("red").unwrap().take() else {
				panic!("the edited color shows");
			};
			assert_eq!(pixels, [0, 0, 255, 255]);

			collection.sources.remove("red");
			assert!(sources.sync(&collection).is_empty());
			assert!(sources.get("red").is_none());

			collection.sources.remove("guest");
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
			let publication = origin.create_broadcast("room/guest").unwrap();
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
			publication.announce(moq_net::origin::Route::default()).unwrap();

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
			let publication = origin.create_broadcast("room/guest").unwrap();
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
			publication.announce(moq_net::origin::Route::default()).unwrap();

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
			let publication = origin.create_broadcast("room/guest").unwrap();
			let mut broadcast = publication.broadcast().clone();
			let mut catalog = moq_mux::catalog::Producer::new(&mut broadcast, Default::default()).unwrap();
			let mut config = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Opus, 48_000, 2);
			config.broadcast = Some(moq_net::path::Relative::new("../other").unwrap());
			catalog
				.mutate(|catalog| {
					catalog.audio.renditions.insert("audio".into(), config);
				})
				.unwrap();
			publication.announce(moq_net::origin::Route::default()).unwrap();

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
		_publication: moq_net::origin::Publication,
		broadcast: moq_net::broadcast::Producer,
		catalog: moq_mux::catalog::Producer,
		video: moq_video::encode::Producer,
		encoder: moq_video::encode::Encoder,
		audio: Option<moq_audio::encode::Producer>,
	}

	/// A video track on its own timestamp base.
	async fn rendition(
		broadcast: &moq_net::broadcast::Producer,
		timebase: moq_mux::catalog::Timebase,
	) -> (moq_video::encode::Producer, moq_video::encode::Encoder) {
		let mut config = moq_video::encode::Config::new(64, 64, moq_video::FrameRate::new(30, 1).unwrap());
		config.kind = moq_video::encode::Kind::Software;
		let rendition = config.probe().await.unwrap();
		let reserved = timebase.reserve().with_interval(config.gop.duration(config.framerate));
		let video = moq_video::encode::Producer::new(broadcast.clone(), reserved, rendition).unwrap();
		(video, moq_video::encode::Encoder::new(&config).unwrap())
	}

	/// A broadcast with a video track, and an audio one with `audio`.
	async fn live(origin: &moq_net::origin::Producer, audio: bool) -> Live {
		let publication = origin.create_broadcast("room/guest").unwrap();
		let mut broadcast = publication.broadcast().clone();
		let catalog = moq_mux::catalog::Producer::new(&mut broadcast, Default::default()).unwrap();
		let (video, encoder) = rendition(&broadcast, catalog.timebase()).await;
		let audio = audio.then(|| {
			let input = moq_audio::encode::Input::default();
			let mut options = moq_audio::encode::Options::default();
			options.config = moq_audio::encode::Config::from_input(moq_audio::encode::Codec::Pcm, &input);
			moq_audio::encode::Producer::new(&mut broadcast, catalog.clone(), input, &options).unwrap()
		});
		publication.announce(moq_net::origin::Route::default()).unwrap();
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
		until(|| match feed.take() {
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
			assert!(feed.take().is_none(), "shown on arrival");
			until_time(due).await;
			assert_eq!(shown(feed).await, Duration::from(stamp));
			assert_eq!(feed.delay(), Some(VIDEO_DELAY));
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
			drop(std::mem::replace(&mut live.video, video));
			until_time(kio::clock::now()).await;
			let arrived = kio::clock::now();
			picture(&mut live.video, &mut encoder, 0);
			let moved = until(|| match feed.take() {
				Some(Picture::Frame(frame)) => panic!("{:?} shown on arrival", frame.timestamp),
				_ => clock.due(first).filter(|at| *at != due),
			})
			.await;
			assert!(moved > due, "the replacement re-anchored behind the first track");
			until_time(arrived + VIDEO_DELAY - Duration::from_millis(1)).await;
			assert!(feed.take().is_none(), "shown before its delay");
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
			assert!(target + crate::playout::BUS_LATENCY > VIDEO_DELAY, "{target:?}");
			let due = arrived + target + crate::playout::BUS_LATENCY;
			until(|| (clock.due(stamp) == Some(due)).then_some(())).await;
			until_time(due - Duration::from_millis(1)).await;
			assert!(feed.take().is_none(), "shown before its sound");
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

			let Some(Picture::Rgba { size, pixels }) = sources.get("logo").unwrap().take() else {
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
			assert!(matches!(title.take(), Some(Picture::Overlay)));
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
			assert!(matches!(feed.take(), Some(Picture::Rgba { pixels, .. }) if pixels == [0; 4]));

			let (broadcast, video, _live) = video(None).await;
			let target = Target {
				broadcast: broadcast.consume(),
				video: Some(video),
				audio: None,
				end: moq_net::Timestamp::from_micros(2_500_000).unwrap(),
			};
			let take = Take::new(&target, Duration::from_secs(1)).unwrap();
			let speed = 0.5;
			let begin = kio::clock::now();
			feed.replay(take, speed).unwrap();

			// From the start, not from the keyframe of its group at 1 s, each
			// picture `t / speed` into the replay, and none sooner. The clock
			// moves only to each picture's moment, which the timer wheel rounds
			// up to its millisecond.
			let start = moq_net::Timestamp::from_micros(1_500_000).unwrap();
			for index in 45..75u64 {
				let timestamp = moq_net::Timestamp::from_micros(index * 1_000_000 / 30).unwrap();
				let due = begin + (Duration::from(timestamp) - Duration::from(start)).div_f32(speed);
				assert!(feed.take().is_none(), "a picture before {timestamp:?} is due");
				until_time(due).await;
				let shown = until(|| match feed.take() {
					Some(Picture::Frame(frame)) => Some(frame.timestamp),
					Some(_) => panic!("cleared before {timestamp:?}"),
					None => None,
				})
				.await;
				assert_eq!(shown, timestamp);
			}

			// Nothing more, and the replay ends once the span's second has
			// played over two.
			assert!(feed.take().is_none(), "the picture at the end shows for no time");
			let end = begin + Duration::from_secs(2);
			until_time(end - Duration::from_millis(1)).await;
			assert!(feed.take().is_none(), "cleared before the end");
			until_time(end).await;
			until(|| matches!(feed.take(), Some(Picture::Rgba { .. })).then_some(())).await;
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
			// Its first picture, at 1.5 s, is due at once.
			feed.replay(take(2_500_000), 1.0).unwrap();
			until(|| matches!(feed.take(), Some(Picture::Frame(_))).then_some(())).await;
			// This one's first picture, at the same 1.5 s, is due 20 ms in.
			feed.replay(take(2_480_000), 1.0).unwrap();
			let shown = until(|| feed.take()).await;
			assert!(matches!(shown, Picture::Rgba { pixels, .. } if pixels == [0; 4]));
		})
		.await;
	}
}
