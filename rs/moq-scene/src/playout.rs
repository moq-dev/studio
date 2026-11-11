//! When a broadcast source's audio sounds and its pictures show: on its
//! publisher's timestamps, so they stay in sync through jitter and stalls.
//!
//! A broadcast's audio and video share one [`timeline`] clock, as they do in
//! `moq play`: audio owns the clock's anchor while it plays, and video shows
//! each picture once the clock says it is due and its sound has played.
//! Without audio, video paces itself [`VIDEO_DELAY`] behind its arrivals.
//!
//! `moq play` holds its jitter buffer in the speaker. Here a [`Speaker`] holds
//! it: it places each frame the playout target moq-audio's decoder estimates
//! after it arrives, and a [`Writer`] hands it to the bus [`WRITE_LEAD`] ahead
//! of then, at the bus position that moment maps to. A media or replay
//! source's sound goes through a [`Writer`] too, so it sounds with the
//! pictures it shows.

use std::time::Duration;

use kio::clock::Instant;
use moq_audio::bus;
use moq_mux::timeline;
use tokio::sync::Notify;

use crate::source::Mixed;

/// How far a broadcast without audio plays behind its arrivals, as under
/// `moq play --delay auto`. With audio, video follows the audio's target.
pub(crate) const VIDEO_DELAY: Duration = Duration::from_millis(100);

/// The longest playout target, the most lateness the jitter estimate can
/// represent, and so how long a stalled group is waited on before it is
/// skipped, as under `moq play --delay auto`. A shorter wait would cap the
/// estimate at it, since a skipped group is never measured.
pub(crate) const MAX_DELAY: Duration = Duration::from_secs(2);

/// How far ahead of its moment a sound is handed to the bus.
///
/// The bus mixes a block at a time, on a clock thread that may wake late, and
/// fails a write any part of which it has already mixed, so a sound is handed
/// over this long before its moment to be mixed whole.
pub(crate) const WRITE_LEAD: Duration = Duration::from_millis(50);

const _: () = assert!(
	WRITE_LEAD.as_nanos() < bus::REACH.as_nanos(),
	"a sound handed over the lead ahead is in the bus's reach"
);

/// How far over its target the queue may run before an arrival is skipped to
/// land back on it, as `moq play`'s speaker allows: one frame of cadence.
const SLACK: Duration = Duration::from_millis(20);

/// How far apart two stamps may be and still meet: a millisecond-stamped
/// ingest rounds each one.
const CONTIGUOUS: Duration = Duration::from_millis(1);

/// Places one broadcast audio track's frames on the bus clock, and reports
/// where they sound to the broadcast's clock, which video follows.
///
/// Dropping it hands the clock's anchor back to video.
pub(crate) struct Speaker<'a> {
	clock: timeline::Producer,
	/// Woken on every frame placed and when the speaker stops, so pictures
	/// held for their sound show once it plays, or once there is none.
	changed: &'a Notify,
	/// The last frame placed: when it finishes sounding, and the media time it
	/// ends at.
	tail: Option<(Instant, Duration)>,
	/// When the media after a skip starts sounding. The clock holds one
	/// mapping, and the audio queued ahead of the skip plays on the old one
	/// until then, so frames placed before it are reported only once it comes:
	/// see [`commit`](Self::commit).
	skipped: Option<Instant>,
	/// The newest frame placed but not reported yet: when it finishes
	/// sounding, the media time it ends at, and the target it was placed on.
	unreported: Option<(Instant, Duration, Duration)>,
}

/// What to do with a frame [`Speaker::place`] was given.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Place {
	/// Play it from this moment.
	Play(Instant),
	/// Drop it: more is queued than the target needs, after a burst or a
	/// target that fell, so the media skips ahead to land back on the target.
	Skip,
}

impl<'a> Speaker<'a> {
	/// Place audio on `clock`, taking its anchor from video on the first frame,
	/// and wake `changed` whenever that may let a picture show.
	pub(crate) fn new(clock: timeline::Producer, changed: &'a Notify) -> Self {
		Self {
			clock,
			changed,
			tail: None,
			skipped: None,
			unreported: None,
		}
	}

	/// Place a decoded frame covering `duration` of media from `start`,
	/// arriving at `now`, given the playout `target` moq-audio estimated.
	///
	/// A frame continues the one before, after silence for any hole in the
	/// media between them. A frame that arrives too late for that, because the
	/// network stalled longer than the target, re-buffers instead: it sounds the
	/// whole target after it arrived, and the clock re-pins on it, so video
	/// waits along with the sound rather than running ahead of it. A frame
	/// that would queue past the target, or that overlaps media already queued,
	/// is skipped, so the queue never holds more than the target.
	pub(crate) fn place(&mut self, start: Duration, duration: Duration, target: Duration, now: Instant) -> Place {
		let target = target.min(MAX_DELAY);
		let end = start.saturating_add(duration);
		let begins = match self.tail {
			None => now + target + WRITE_LEAD,
			Some((sounds, media)) => {
				let follows = sounds + start.saturating_sub(media);
				// A write half its lead late is still ahead of the mix, however
				// coarsely the bus advances.
				if follows < now + WRITE_LEAD / 2 {
					self.clock.audio_restarted();
					self.skipped = None;
					self.unreported = None;
					now + target + WRITE_LEAD
				} else if start.saturating_add(CONTIGUOUS) < media
					|| follows.saturating_duration_since(now) > target + WRITE_LEAD + SLACK
				{
					// Media overlapping what is queued, or past the target, would
					// queue without bound however fast it arrives.
					self.tail = Some((sounds, media.max(end)));
					self.skipped = Some(sounds);
					return Place::Skip;
				} else {
					follows
				}
			}
		};
		let sounds = begins + duration;
		self.tail = Some((sounds, end));
		self.unreported = Some((sounds, end, target));
		if self.skipped.is_none_or(|skipped| skipped <= now) {
			self.report(now);
		}
		Place::Play(begins)
	}

	/// When [`commit`](Self::commit) has a report to make, after a skip.
	pub(crate) fn deferred(&self) -> Option<Instant> {
		self.unreported.and(self.skipped)
	}

	/// Report the frames placed since a skip, once the media after it sounds at
	/// `now`, whether or not another frame has arrived since.
	pub(crate) fn commit(&mut self, now: Instant) {
		if self.deferred().is_some_and(|skipped| skipped <= now) {
			self.report(now);
		}
	}

	/// Tell the clock where the newest frame placed sounds, as of `now`.
	fn report(&mut self, now: Instant) {
		self.skipped = None;
		let Some((sounds, end, target)) = self.unreported.take() else {
			return;
		};
		let audio = timeline::Audio {
			end,
			buffered: sounds.saturating_duration_since(now),
			target,
		};
		self.clock.audio(audio, now);
		self.changed.notify_one();
	}
}

impl Drop for Speaker<'_> {
	fn drop(&mut self) {
		self.clock.audio_stopped();
		self.changed.notify_one();
	}
}

/// Whether a picture stamped `timestamp` may show at `now` on `schedule`:
/// while audio plays, only once the sound stamped with it has played, so a
/// stall in the audio alone holds the picture too.
pub(crate) fn heard(schedule: &timeline::Consumer, timestamp: moq_net::Timestamp, now: Instant) -> bool {
	schedule.target().is_none()
		|| schedule
			.playhead(now)
			.is_some_and(|head| Duration::from(timestamp) <= head + CONTIGUOUS)
}

/// Writes placed frames into a positioned bus channel, each at the bus
/// position its moment maps to, so it sounds exactly then.
///
/// The bus's [`Mixer`](crate::source::Mixer) advances it sample-exact on the clock from its start,
/// so a moment `t` is bus position `(t - start) * rate`, and the bus has never
/// mixed past the position of the clock's reading.
pub(crate) struct Writer {
	channel: bus::Positioned,
	/// Bytes per sample frame of the channel's `f32` input.
	stride: usize,
	rate: u32,
	mixed: Mixed,
	/// The bus position the last write ended at, which the next follows.
	end: i128,
}

impl Writer {
	/// Write into `channel` on the bus `mixed` follows, taking interleaved
	/// `f32` at the bus `rate` in `channels`.
	pub(crate) fn new(channel: bus::Positioned, mixed: Mixed, rate: u32, channels: u32) -> Self {
		Self {
			channel,
			stride: channels as usize * size_of::<f32>(),
			rate,
			mixed,
			end: 0,
		}
	}

	/// Write `data` to sound at `sounds`, handed over [`WRITE_LEAD`] ahead.
	///
	/// A frame that starts within [`CONTIGUOUS`] of where the last one ended
	/// continues it. The part already mixed, or overlapping the last frame, is
	/// cut, so the rest still sounds on schedule. The part past what the bus
	/// holds waits until the bus has mixed far enough to hold it, however far
	/// behind the clock it runs. Once the bus's mixer is gone, nothing mixes,
	/// and what is left is dropped.
	pub(crate) async fn write(&mut self, sounds: Instant, data: &[u8]) -> Result<(), moq_audio::Error> {
		let mut position = self.position(sounds);
		if position.abs_diff(self.end) <= self.frames(CONTIGUOUS) {
			position = self.end;
		}
		let end = position + (data.len() / self.stride) as i128;
		let reach = self.frames(bus::REACH) as i128;
		loop {
			let from = position.max(self.end).max(self.ceiling(kio::clock::now()));
			if from >= end {
				return Ok(());
			}
			// The bus may have mixed further since, which only adds room.
			let next = *self.mixed.position.borrow_and_update() as i128;
			let until = end.min(next + reach);
			if from < until {
				let bytes = |at: i128| (at - position) as usize * self.stride;
				self.channel.write(from as u64, &data[bytes(from)..bytes(until)])?;
				self.end = until;
			}
			if until >= end || self.mixed.position.changed().await.is_err() {
				return Ok(());
			}
		}
	}

	/// The bus position sounding at `at`, to the nearest frame.
	fn position(&self, at: Instant) -> i128 {
		let start = self.mixed.start;
		let nanos = match at.checked_duration_since(start) {
			Some(after) => after.as_nanos() as i128,
			None => -(start.duration_since(at).as_nanos() as i128),
		};
		(nanos * i128::from(self.rate) + 500_000_000).div_euclid(1_000_000_000)
	}

	/// The first bus position the bus cannot have mixed by `now`: rounded up,
	/// since it mixes a frame only once the clock has passed it.
	fn ceiling(&self, now: Instant) -> i128 {
		let nanos = now.saturating_duration_since(self.mixed.start).as_nanos();
		(nanos * u128::from(self.rate)).div_ceil(1_000_000_000) as i128
	}

	/// The whole frames `duration` spans.
	fn frames(&self, duration: Duration) -> u128 {
		duration.as_nanos() * u128::from(self.rate) / 1_000_000_000
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::source::Mixer;

	const FRAME: Duration = Duration::from_millis(20);
	const TARGET: Duration = Duration::from_millis(100);

	fn at(millis: u64) -> Instant {
		Instant::ZERO + Duration::from_secs(10) + Duration::from_millis(millis)
	}

	fn ms(millis: u64) -> Duration {
		Duration::from_millis(millis)
	}

	fn stamp(millis: u64) -> moq_net::Timestamp {
		moq_net::Timestamp::from_millis(millis).unwrap()
	}

	/// The first frame waits out the target and the bus's latency, later ones
	/// follow it on their stamps however unevenly they arrive within it, and a
	/// picture is due when the sound stamped with it plays.
	#[test]
	fn video_is_due_when_its_audio_sounds() {
		let clock = timeline::Producer::new(VIDEO_DELAY);
		let changed = Notify::new();
		let mut speaker = Speaker::new(clock.clone(), &changed);
		assert_eq!(speaker.place(ms(1_000), FRAME, TARGET, at(0)), Place::Play(at(150)));
		assert_eq!(clock.due(stamp(1_000)), Some(at(150)));
		// Late within the target, and early: both keep their place.
		assert_eq!(speaker.place(ms(1_020), FRAME, TARGET, at(90)), Place::Play(at(170)));
		assert_eq!(speaker.place(ms(1_040), FRAME, TARGET, at(91)), Place::Play(at(190)));
		assert_eq!(clock.due(stamp(1_040)), Some(at(190)));
		assert_eq!(clock.consume().target(), Some(TARGET));

		// A stopped track hands the clock back to video.
		drop(speaker);
		assert_eq!(clock.consume().target(), None);
	}

	/// A picture whose sound has not arrived waits for it, even once due: the
	/// audio stalled, and the picture stalls with it.
	#[test]
	fn a_picture_waits_for_its_sound_to_play() {
		let clock = timeline::Producer::new(VIDEO_DELAY);
		let schedule = clock.consume();
		assert!(
			heard(&schedule, stamp(5_000), at(0)),
			"without audio, only the deadline counts"
		);
		let changed = Notify::new();
		let mut speaker = Speaker::new(clock.clone(), &changed);
		speaker.place(ms(1_000), FRAME, TARGET, at(0));
		assert!(!heard(&schedule, stamp(1_000), at(148)));
		assert!(heard(&schedule, stamp(1_000), at(150)));
		assert!(clock.due(stamp(1_100)).is_some_and(|due| due < at(500)));
		assert!(heard(&schedule, stamp(1_020), at(500)));
		assert!(!heard(&schedule, stamp(1_100), at(500)), "shown past its sound");
	}

	/// A stall longer than the target re-buffers: the next frame sounds the
	/// target after it arrived, and the pictures stamped with it wait as long.
	#[test]
	fn a_stall_holds_video_back_with_the_sound() {
		let clock = timeline::Producer::new(VIDEO_DELAY);
		let changed = Notify::new();
		let mut speaker = Speaker::new(clock.clone(), &changed);
		speaker.place(ms(1_000), FRAME, TARGET, at(0));
		assert_eq!(speaker.place(ms(1_020), FRAME, TARGET, at(500)), Place::Play(at(650)));
		assert_eq!(clock.due(stamp(1_020)), Some(at(650)));
	}

	/// Lost media plays as the silence it was, so the pictures after it keep
	/// their moments; a hole past the target skips instead, and the clock
	/// re-pins on the frame after it.
	#[test]
	fn a_hole_plays_as_silence_up_to_the_target() {
		let clock = timeline::Producer::new(VIDEO_DELAY);
		let changed = Notify::new();
		let mut speaker = Speaker::new(clock.clone(), &changed);
		speaker.place(ms(1_000), FRAME, TARGET, at(0));
		assert_eq!(speaker.place(ms(1_060), FRAME, TARGET, at(40)), Place::Play(at(210)));
		assert_eq!(clock.due(stamp(1_060)), Some(at(210)));

		// 1 s lost, with only the target queued: skipped back onto the target.
		// The audio queued ahead of the skip keeps its pictures' moments until
		// the media after it starts to sound.
		assert_eq!(speaker.place(ms(2_080), FRAME, TARGET, at(60)), Place::Skip);
		let schedule = clock.consume();
		assert_eq!(speaker.place(ms(2_100), FRAME, TARGET, at(61)), Place::Play(at(230)));
		assert_eq!(clock.due(stamp(1_060)), Some(at(210)));
		assert!(!heard(&schedule, stamp(2_100), at(61)));
		// It sounds whether or not anything arrives after it.
		assert_eq!(speaker.deferred(), Some(at(230)));
		speaker.commit(at(229));
		assert_eq!(clock.due(stamp(1_060)), Some(at(210)));
		speaker.commit(at(230));
		assert_eq!(speaker.deferred(), None);
		assert_eq!(clock.due(stamp(2_100)), Some(at(230)));
		assert!(heard(&schedule, stamp(2_100), at(230)));
	}

	/// A burst queues no more than the target and a frame of slack: the rest
	/// is skipped, and play goes on from what arrives next.
	#[test]
	fn a_burst_skips_past_the_target() {
		let clock = timeline::Producer::new(VIDEO_DELAY);
		let changed = Notify::new();
		let mut speaker = Speaker::new(clock.clone(), &changed);
		let placed = (0..10)
			.map(|index| speaker.place(ms(1_000 + index * 20), FRAME, TARGET, at(0)))
			.filter(|place| *place != Place::Skip)
			.collect::<Vec<_>>();
		assert_eq!(placed, [Place::Play(at(150)), Place::Play(at(170))]);
		// What arrives next follows the queue, and the clock takes its media
		// once the first of it sounds, on arrival or not.
		for index in 0..9 {
			let place = speaker.place(ms(1_200 + index * 20), FRAME, TARGET, at(20 + index * 20));
			assert_eq!(place, Place::Play(at(190 + index * 20)));
			assert_eq!(clock.due(stamp(1_000)), Some(at(150)), "{index}");
		}
		assert_eq!(speaker.place(ms(1_380), FRAME, TARGET, at(200)), Place::Play(at(370)));
		assert_eq!(clock.due(stamp(1_200)), Some(at(190)));
		assert_eq!(speaker.deferred(), None);
	}

	/// Media overlapping what is queued, stamped the same or a millisecond
	/// apart however long each frame is, is skipped rather than queued behind
	/// it; once the queue runs out, a rewound timeline plays from the target.
	#[test]
	fn overlapping_media_queues_no_more_than_the_target() {
		let clock = timeline::Producer::new(VIDEO_DELAY);
		let changed = Notify::new();
		let mut speaker = Speaker::new(clock.clone(), &changed);
		for index in 0..1_000 {
			for start in [ms(1_000), ms(1_000 + index)] {
				if let Place::Play(begins) = speaker.place(start, FRAME, TARGET, at(0)) {
					assert!(begins <= at(0) + TARGET + WRITE_LEAD + SLACK, "{index}: {begins:?}");
				}
			}
		}
		assert_eq!(speaker.place(ms(500), FRAME, TARGET, at(1_000)), Place::Play(at(1_150)));
		assert_eq!(clock.due(stamp(500)), Some(at(1_150)));
	}

	/// The bus mixed a block at a time along with a paused clock, as the
	/// program's audio clock mixes it, noting each change in what one mix heard.
	struct Ear {
		mixer: Mixer,
		reader: bus::Reader,
		/// The bus position of the next frame mixed.
		next: u64,
		start: Instant,
		rate: u64,
		/// Each bus position the first channel changed at, and what to.
		heard: Vec<(u64, f32)>,
	}

	impl Ear {
		const BLOCK: u64 = 480;

		/// Mix every block the clock passes up to `until`, as it passes each.
		async fn until(&mut self, until: Instant) {
			loop {
				let next = self.next + Self::BLOCK;
				let due = self.start + Duration::from_nanos(next * 1_000_000_000 / self.rate);
				if due > until {
					return;
				}
				kio::clock::sleep_until(due).await;
				self.mixer.advance(Self::BLOCK as usize);
				self.next = next;
				let std::task::Poll::Ready(Some(read)) = self.reader.poll_read(&kio::Waiter::noop()) else {
					panic!("an advance publishes a block");
				};
				let mut last = self.heard.last().map_or(0.0, |(_, value)| *value);
				for (index, sample) in read.data.chunks(2).enumerate() {
					if sample[0] != last {
						last = sample[0];
						self.heard.push((read.position + index as u64, last));
					}
				}
			}
		}
	}

	/// A frame sounds at the bus position its moment maps to, sample-exact:
	/// handed over ahead, continuing the last across a stamp's rounding,
	/// overlapping it, late, wholly late, while the bus lags the clock a little
	/// and far past what it holds, and longer than the bus holds. Whatever
	/// cannot sound when it says is cut.
	#[tokio::test(start_paused = true)]
	async fn a_write_sounds_at_its_position() {
		let (bus, driver) = bus::Bus::new(Default::default()).unwrap();
		let config = bus.config().clone();
		let strip = bus::Strip::default();
		let mut input = moq_audio::encode::Input::new(config.sample_rate, config.layout);
		input.format = moq_audio::Format::F32;
		let channel = bus.positioned(input, strip.clone()).unwrap();
		let mix = bus.mix();
		mix.route(&strip);
		let start = kio::clock::now();
		let (mixer, mixed) = Mixer::new(driver, start);
		let mut writer = Writer::new(channel, mixed, config.sample_rate, config.layout.channels());
		let mut ear = Ear {
			mixer,
			next: 0,
			reader: mix.reader(),
			start,
			rate: u64::from(config.sample_rate),
			heard: Vec::new(),
		};
		let at = |millis: u64| start + ms(millis);
		// `millis` of a frame of `value`, at 48 frames a millisecond.
		let frame = |value: f32, millis: usize| bytemuck::cast_slice::<f32, u8>(&vec![value; 2 * 48 * millis]).to_vec();

		ear.until(at(10)).await;
		writer.write(at(60), &frame(1.0, 20)).await.unwrap();
		ear.until(at(30)).await;
		// Stamped half a millisecond after the last ended.
		writer
			.write(at(80) + Duration::from_micros(500), &frame(2.0, 20))
			.await
			.unwrap();
		// Overlapping the last by 5 ms.
		writer.write(at(95), &frame(3.0, 20)).await.unwrap();

		ear.until(at(1_000)).await;
		writer.write(at(990), &frame(4.0, 20)).await.unwrap();
		ear.until(at(2_000)).await;
		writer.write(at(1_980), &frame(5.0, 20)).await.unwrap();

		// The bus stalls 30 ms behind the clock, then catches up.
		ear.until(at(2_970)).await;
		kio::clock::sleep_until(at(3_000)).await;
		writer.write(at(3_050), &frame(6.0, 20)).await.unwrap();
		ear.until(at(3_200)).await;

		// Longer than the bus holds, handed over as the bus mixes.
		ear.until(at(4_000)).await;
		let long = frame(7.0, 300);
		let (written, ()) = tokio::join!(writer.write(at(4_050), &long), ear.until(at(4_500)));
		written.unwrap();

		// The bus stalls half a second behind the clock: the write waits for
		// it to catch up, rather than landing past what it holds.
		kio::clock::sleep_until(at(5_000)).await;
		let short = frame(8.0, 20);
		let (written, ()) = tokio::join!(writer.write(at(5_050), &short), ear.until(at(5_200)));
		written.unwrap();

		assert_eq!(
			ear.heard,
			[
				(48 * 60, 1.0),
				(48 * 80, 2.0),
				(48 * 100, 3.0),
				(48 * 115, 0.0),
				(48 * 1_000, 4.0),
				(48 * 1_010, 0.0),
				(48 * 3_050, 6.0),
				(48 * 3_070, 0.0),
				(48 * 4_050, 7.0),
				(48 * 4_350, 0.0),
				(48 * 5_050, 8.0),
				(48 * 5_070, 0.0),
			]
		);
	}
}
