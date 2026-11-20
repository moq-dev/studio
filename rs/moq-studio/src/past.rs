//! The last 30 seconds of what REVIEW replays, as pictures to scrub.
//!
//! Nothing records for it. About once a second while the page is shown, a
//! thread of its own takes the window the way a replay does, from the groups
//! the tracks keep for late viewers anyway, and decodes the keyframe each
//! group opens with, small and on the CPU, once per keyframe. A picture is
//! the moment its group starts, so the strip is as fine as the track's
//! keyframe interval. The sound is decoded too, once per group, down to the
//! loudest sample of each packet.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::Poll;
use std::time::Duration;

use anyhow::Context;
use kio::clock::Instant;
use moq_mux::container::Container as _;

use crate::engine::Engine;
use crate::replay::{self, Of};

/// How often the window is taken again while it is watched.
const PERIOD: Duration = Duration::from_secs(1);

/// How long the thread keeps going once the page stops asking.
const LINGER: Duration = Duration::from_secs(2);

/// The last [`Picture::id`] handed out, shared by every worker the page
/// starts, since the window keeps textures across them.
static LAST_ID: AtomicU64 = AtomicU64::new(0);

/// How wide a picture is decoded: enough for the page's large picture.
const WIDTH: u32 = 480;

/// The recent past of one thing REVIEW replays, kept fresh while watched.
pub struct Past {
	shared: Arc<Shared>,
}

struct Shared {
	engine: Arc<Engine>,
	watch: Mutex<Watch>,
	/// Wakes the thread early, when what it watches changes.
	wake: Condvar,
}

struct Watch {
	of: Of,
	/// When the page last asked, after which the thread lingers and stops.
	seen: Instant,
	running: bool,
	view: Arc<View>,
}

/// What the window held when last taken.
pub struct View {
	/// What it is of, which lags a change of pick by a sample.
	pub of: Of,
	/// The keyframe each group opens with, oldest first, or why they could
	/// not be read.
	pub pictures: Result<Vec<Arc<Picture>>, String>,
	/// The loudest sample of each packet of sound, from 0 to 1, oldest first,
	/// or why the sound could not be read.
	pub sound: Result<Vec<(Instant, f32)>, String>,
	/// How far back it reaches, or why there is nothing to replay. Pictures
	/// or sound that cannot be read for the strip leave it held, since a
	/// replay or clip copies what is held without decoding it here.
	pub held: Option<Result<Held, String>>,
}

impl Default for View {
	fn default() -> Self {
		Self {
			of: Of::Program,
			pictures: Ok(Vec::new()),
			sound: Ok(Vec::new()),
			held: None,
		}
	}
}

/// How much of the window a sample found.
#[derive(Clone, Copy, Debug)]
pub struct Held {
	/// The oldest moment held.
	pub oldest: Instant,
	/// Whether it holds pictures, rather than sound alone.
	pub video: bool,
}

/// A decoded keyframe.
pub struct Picture {
	/// Unique for as long as the studio runs, so the window uploads it once.
	pub id: u64,
	/// When it was shown.
	pub at: Instant,
	pub size: [usize; 2],
	/// Tightly packed RGBA.
	pub rgba: Vec<u8>,
}

impl Past {
	pub fn new(engine: Arc<Engine>) -> Self {
		let watch = Watch {
			of: Of::Program,
			seen: Instant::now(),
			running: false,
			view: Arc::default(),
		};
		Self {
			shared: Arc::new(Shared {
				engine,
				watch: Mutex::new(watch),
				wake: Condvar::new(),
			}),
		}
	}

	/// Keep the past of `of` fresh, for a frame that shows it, and what the
	/// last sample found.
	pub fn watch(&self, of: &Of) -> Arc<View> {
		let mut watch = self.shared.watch.lock().unwrap();
		watch.seen = Instant::now();
		if watch.of != *of {
			watch.of = of.clone();
			self.shared.wake.notify_all();
		}
		if !watch.running {
			watch.running = true;
			let shared = self.shared.clone();
			let spawned = std::thread::Builder::new()
				.name("review-past".into())
				.spawn(move || run(&shared));
			if let Err(err) = spawned {
				watch.running = false;
				watch.view = Arc::new(View {
					of: of.clone(),
					held: Some(Err(format!("cannot start the strip's decoder: {err}"))),
					..View::default()
				});
			}
		}
		watch.view.clone()
	}

	/// Show `view` as if sampled, for the window's views.
	#[cfg(test)]
	pub fn seed(&self, view: View) {
		let mut watch = self.shared.watch.lock().unwrap();
		// A thread that runs would replace it.
		watch.running = true;
		watch.of = view.of.clone();
		watch.view = Arc::new(view);
	}
}

/// Take the window of `of` once, on a decoder of its own, as the thread does.
#[cfg(test)]
pub fn sample(engine: &Engine, of: &Of) -> View {
	Decoding::default().sample(engine, of)
}

/// Take the window about once a [`PERIOD`] while watched.
fn run(shared: &Shared) {
	let mut decoding = Decoding::default();
	let mut watch = shared.watch.lock().unwrap();
	loop {
		if watch.seen.elapsed() > LINGER {
			watch.running = false;
			return;
		}
		let of = watch.of.clone();
		drop(watch);
		let view = decoding.sample(&shared.engine, &of);
		watch = shared.watch.lock().unwrap();
		watch.view = Arc::new(view);
		if watch.of == of {
			watch = shared.wake.wait_timeout(watch, PERIOD).unwrap().0;
		}
	}
}

/// The decoder and the pictures already decoded, of one rendition.
#[derive(Default)]
struct Decoding {
	/// What they are of: the pick, and the rendition's name and config.
	key: Option<(Of, String, hang::catalog::VideoConfig)>,
	decoder: Option<moq_video::decode::Decoder>,
	/// By the keyframe's timestamp, in microseconds on its broadcast's timeline.
	pictures: BTreeMap<u128, Arc<Picture>>,
	/// The same for the sound: what it is of, its decoder, and each group's
	/// peaks by the group's start.
	sound_key: Option<(Of, String, hang::catalog::AudioConfig)>,
	sound_decoder: Option<moq_audio::decode::Decoder>,
	peaks: BTreeMap<u64, Vec<(moq_net::Timestamp, f32)>>,
}

impl Decoding {
	fn sample(&mut self, engine: &Engine, of: &Of) -> View {
		let now = Instant::now();
		match replay::take(engine, of, Duration::ZERO, replay::WINDOW, true) {
			Ok(take) => self.read(of, &take, now),
			Err(err) => {
				self.reset();
				View {
					of: of.clone(),
					held: Some(Err(format!("{err:#}"))),
					..View::default()
				}
			}
		}
	}

	/// What `take`, of `of` and ending `now`, holds.
	fn read(&mut self, of: &Of, take: &moq_scene::replay::Take, now: Instant) -> View {
		let mut view = View {
			of: of.clone(),
			..View::default()
		};
		let end = match Duration::try_from(take.end) {
			Ok(end) => end,
			Err(err) => {
				self.reset();
				view.held = Some(Err(format!("the take's end: {err}")));
				return view;
			}
		};
		// The moment a timestamp of the take's broadcast was shown.
		let at = |timestamp: moq_net::Timestamp| -> Result<Instant, moq_net::TimeOverflow> {
			Ok(now.saturating_sub(end.saturating_sub(Duration::try_from(timestamp)?)))
		};
		let broadcast = take.consume();
		let mut oldest = None;
		if let Some((name, config)) = &take.audio {
			view.sound = self
				.sound(&broadcast, of, name, config, &at)
				.map_err(|err| format!("{err:#}"));
			oldest = view
				.sound
				.as_ref()
				.ok()
				.and_then(|sound| sound.first())
				.map(|(at, _)| *at);
		}
		if let Some((name, config)) = &take.video {
			view.pictures = self
				.pictures(&broadcast, of, name, config, &at)
				.map_err(|err| format!("{err:#}"));
			let first = view
				.pictures
				.as_ref()
				.ok()
				.and_then(|pictures| pictures.first())
				.map(|picture| picture.at);
			oldest = oldest.into_iter().chain(first).min();
		}
		view.held = Some(Ok(Held {
			oldest: oldest.unwrap_or(now),
			video: take.video.is_some(),
		}));
		view
	}

	/// The keyframe of each group of the video `name`, decoding those not
	/// decoded yet and letting go of those the window no longer holds.
	fn pictures(
		&mut self,
		broadcast: &moq_net::broadcast::Consumer,
		of: &Of,
		name: &str,
		config: &hang::catalog::VideoConfig,
		at: &impl Fn(moq_net::Timestamp) -> Result<Instant, moq_net::TimeOverflow>,
	) -> anyhow::Result<Vec<Arc<Picture>>> {
		let key = Some((of.clone(), name.to_string(), config.clone()));
		if self.key != key {
			self.reset();
			self.key = key;
		}
		let container = moq_mux::catalog::hang::Container::try_from(config)?;
		let mut kept = BTreeMap::new();
		for frame in firsts(broadcast, name, container)? {
			let micros = frame.timestamp.as_micros()?;
			let picture = match self.pictures.remove(&micros) {
				Some(picture) => picture,
				None => {
					let Some(rgba) = self.decode(config, &frame)? else {
						continue;
					};
					let size = rgba.size();
					Arc::new(Picture {
						id: LAST_ID.fetch_add(1, Ordering::Relaxed) + 1,
						at: at(frame.timestamp)?,
						size: [size.width as usize, size.height as usize],
						rgba: rgba.into_data(),
					})
				}
			};
			kept.insert(micros, picture);
		}
		self.pictures = kept;
		Ok(self.pictures.values().cloned().collect())
	}

	/// Decode one keyframe on its own, small, or `None` if the decoder gave
	/// back no picture for it.
	fn decode(
		&mut self,
		config: &hang::catalog::VideoConfig,
		frame: &moq_mux::container::Frame,
	) -> anyhow::Result<Option<moq_video::convert::Rgba>> {
		let decoder = match &mut self.decoder {
			Some(decoder) => decoder,
			None => {
				let mut options = moq_video::decode::Config::new();
				options.output = moq_video::Output::Cpu;
				let decoder = moq_video::decode::Decoder::new(config, &options)
					.context("cannot decode the pictures for the strip")?;
				self.decoder.insert(decoder)
			}
		};
		decoder.decode(&frame.payload, frame.timestamp, true)?;
		// Drained at once, so the keyframe comes out without the rest of its group.
		decoder.flush()?;
		let mut picture = None;
		while let Some(decoded) = decoder.receive()? {
			picture = Some(decoded);
		}
		let Some(picture) = picture else { return Ok(None) };
		let size = picture.size();
		let height = (u64::from(WIDTH) * u64::from(size.height) / u64::from(size.width.max(1))) as u32;
		let small = moq_video::Size::new(WIDTH, (height.max(2) + 1) & !1);
		let mut resize = moq_video::resize::Config::default();
		resize.output = moq_video::Output::Cpu;
		let small = picture.resize(small, &resize)?;
		Ok(Some(small.surface.to_rgba(&moq_video::convert::Config::new())?))
	}

	/// The loudest sample of each packet of the audio `name`, decoding the
	/// groups not decoded yet, and the newest again, which may have grown.
	fn sound(
		&mut self,
		broadcast: &moq_net::broadcast::Consumer,
		of: &Of,
		name: &str,
		config: &hang::catalog::AudioConfig,
		at: &impl Fn(moq_net::Timestamp) -> Result<Instant, moq_net::TimeOverflow>,
	) -> anyhow::Result<Vec<(Instant, f32)>> {
		let key = Some((of.clone(), name.to_string(), config.clone()));
		if self.sound_key != key {
			self.sound_decoder = None;
			self.peaks.clear();
			self.sound_key = key;
		}
		let container = moq_mux::catalog::hang::Container::try_from(config)?;
		let groups = groups(broadcast, name, false)?;
		let newest = groups.last().map(|(start, _)| *start);
		let mut kept = BTreeMap::new();
		for (start, frames) in groups {
			let peaks = match self.peaks.remove(&start) {
				Some(peaks) if Some(start) != newest => peaks,
				_ => {
					let decoder = match &mut self.sound_decoder {
						Some(decoder) => decoder,
						None => self.sound_decoder.insert(
							moq_audio::decode::Decoder::new(config, &moq_audio::decode::Config::new())
								.context("cannot decode the sound for the strip")?,
						),
					};
					let mut peaks = Vec::new();
					for frame in frames {
						for packet in container.decode(frame)? {
							decoder.decode(&packet.payload)?;
							let mut peak = 0.0_f32;
							while let Some(decoded) = decoder.receive()? {
								peak = decoded.samples.iter().fold(peak, |peak, sample| peak.max(sample.abs()));
							}
							peaks.push((packet.timestamp, peak.min(1.0)));
						}
					}
					peaks
				}
			};
			kept.insert(start, peaks);
		}
		self.peaks = kept;
		Ok(self
			.peaks
			.values()
			.flatten()
			.map(|(timestamp, peak)| Ok((at(*timestamp)?, *peak)))
			.collect::<Result<_, moq_net::TimeOverflow>>()?)
	}

	fn reset(&mut self) {
		self.key = None;
		self.decoder = None;
		self.pictures.clear();
		self.sound_key = None;
		self.sound_decoder = None;
		self.peaks.clear();
	}
}

/// The frame each group of the track `name` opens with, oldest first, as far
/// as the take holds them now.
fn firsts(
	broadcast: &moq_net::broadcast::Consumer,
	name: &str,
	container: moq_mux::catalog::hang::Container,
) -> anyhow::Result<Vec<moq_mux::container::Frame>> {
	let mut firsts = Vec::new();
	for (_, frames) in groups(broadcast, name, true)? {
		let Some(frame) = frames.into_iter().next() else {
			continue;
		};
		// A gap's marker has an empty payload, and is no picture.
		if let Some(first) = container.decode(frame)?.into_iter().next()
			&& !first.payload.is_empty()
		{
			firsts.push(first);
		}
	}
	Ok(firsts)
}

/// Each group of the track `name` by its start, oldest first, with its
/// frames, or only its first if `first`, as far as the take holds them now.
fn groups(
	broadcast: &moq_net::broadcast::Consumer,
	name: &str,
	first: bool,
) -> anyhow::Result<Vec<(u64, Vec<moq_net::frame::Frame>)>> {
	let subscription = moq_net::track::Subscription::default()
		.with_delay(moq_net::track::Delay::default().with_max(None))
		.with_order(moq_net::track::Order::Ascending);
	// A take's tracks are written and finished before it is handed out, so
	// everything is ready and nothing is waited on.
	let now = kio::Waiter::noop();
	let mut source = match broadcast.track(name)?.subscribe(subscription).poll_ok(&now) {
		Poll::Ready(source) => source?,
		Poll::Pending => return Ok(Vec::new()),
	};
	let mut groups = BTreeMap::new();
	while let Poll::Ready(group) = source.poll_recv_group(&now) {
		let Some(mut group) = group? else { break };
		let mut frames = Vec::new();
		while let Poll::Ready(Ok(Some(frame))) = group.poll_read_frame(&now) {
			frames.push(frame);
			if first {
				break;
			}
		}
		groups.insert(group.start, frames);
	}
	Ok(groups.into_iter().collect())
}

// The software encoder is the one every test machine has.
#[cfg(all(test, feature = "openh264"))]
mod tests {
	use super::*;

	/// Three seconds of gray, a keyframe a second, as a take holds them.
	async fn gray() -> moq_scene::replay::Take {
		let mut broadcast = moq_net::broadcast::Info::new().produce();
		let catalog = moq_mux::catalog::Producer::new(&mut broadcast, Default::default()).unwrap();
		let mut config = moq_video::encode::Config::new(64, 64, moq_video::FrameRate::new(30, 1).unwrap());
		config.kind = moq_video::encode::Kind::Software;
		let rendition = config.probe().await.unwrap();
		let reserved = catalog.reserve().with_interval(config.effective_join());
		let track = broadcast
			.unique_track(".avc3", reserved.track_info(hang::catalog::PRIORITY.video))
			.unwrap();
		let mut producer = moq_video::encode::Producer::new(track, reserved, rendition).unwrap();
		let mut encoder = moq_video::encode::Encoder::new(&config).unwrap();
		let gray = vec![0x80; 64 * 64 * 4];
		let at = |index: u64| moq_net::Timestamp::from_micros(index * 1_000_000 / 30).unwrap();
		for index in 0..90u64 {
			if index % 30 == 0 {
				encoder.cut().unwrap();
			}
			let surface = moq_video::Surface::rgba(&gray, moq_video::Size::new(64, 64)).unwrap();
			encoder.encode(&moq_video::Frame::new(surface, at(index))).unwrap();
			let unit = encoder.receive().unwrap().expect("an access unit per picture");
			producer.publish(&unit).unwrap();
		}
		let video = catalog.snapshot().video.renditions.into_iter().next();
		let target = moq_scene::replay::Target {
			broadcast: broadcast.consume(),
			video,
			audio: None,
			end: at(89),
		};
		moq_scene::replay::Take::new(&target, Duration::from_secs(3)).unwrap()
	}

	/// The strip holds the keyframe each group of a take opens with, at the
	/// moment it was shown, and decodes each only once across samples.
	#[tokio::test]
	async fn the_strip_shows_each_groups_keyframe_once() {
		let take = gray().await;
		let (name, video) = take.video.clone().unwrap();

		let zero = Instant::now();
		let shown = |timestamp: moq_net::Timestamp| -> Result<Instant, moq_net::TimeOverflow> {
			Ok(zero.saturating_add(Duration::try_from(timestamp)?))
		};
		let mut decoding = Decoding::default();
		let pictures = decoding
			.pictures(&take.consume(), &Of::Program, &name, &video, &shown)
			.unwrap();
		let secs = |seconds| zero.saturating_add(Duration::from_secs(seconds));
		let ats: Vec<_> = pictures.iter().map(|picture| picture.at).collect();
		assert_eq!(ats, [secs(0), secs(1), secs(2)]);

		let again = decoding
			.pictures(&take.consume(), &Of::Program, &name, &video, &shown)
			.unwrap();
		let ids = |pictures: &[Arc<Picture>]| pictures.iter().map(|picture| picture.id).collect::<Vec<_>>();
		assert_eq!(ids(&again), ids(&pictures), "decoded again");
	}

	/// Pictures the strip cannot decode leave the past held: a replay or a
	/// clip copies it without decoding it here, so neither is refused.
	#[tokio::test]
	async fn pictures_it_cannot_decode_leave_the_past_held() {
		let mut take = gray().await;
		let (_, video) = take.video.as_mut().unwrap();
		// avc1, whose parameter sets must come from a description, and this
		// one is no avcC.
		let hang::catalog::VideoCodec::H264(h264) = &mut video.codec else {
			unreachable!("the software encoder makes H.264")
		};
		h264.inline = false;
		video.description = Some(bytes::Bytes::from_static(&[0xff]));
		let view = Decoding::default().read(&Of::Program, &take, Instant::now());
		let held = view.held.expect("sampled").expect("the past is held");
		assert!(held.video);
		let Err(why) = view.pictures else {
			panic!("the strip cannot decode it")
		};
		assert!(why.starts_with("cannot decode the pictures for the strip"), "{why}");
	}
}
