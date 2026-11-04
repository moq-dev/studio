//! When a broadcast source's audio sounds and its pictures show: on its
//! publisher's timestamps, so they stay in sync through jitter and stalls.
//!
//! A broadcast's audio and video share one [`timeline`] clock, as they do in
//! `moq play`: audio owns the clock's anchor while it plays, and video shows
//! each picture once the clock says it is due and its sound has played.
//! Without audio, video paces itself [`VIDEO_DELAY`] behind its arrivals.
//!
//! `moq play` holds its jitter buffer in the speaker. The bus holds only
//! [`BUS_LATENCY`] of a source, so a [`Speaker`] holds the rest: it places each
//! frame the playout target moq-audio's decoder estimates after it arrives,
//! and a [`Writer`] writes it the bus's latency ahead of then.

use std::time::Duration;

use kio::clock::Instant;
use moq_audio::bus;
use moq_mux::timeline;
use tokio::sync::Notify;

/// How far a broadcast without audio plays behind its arrivals, as under
/// `moq play --delay auto`. With audio, video follows the audio's target.
pub(crate) const VIDEO_DELAY: Duration = Duration::from_millis(100);

/// The longest playout target, the most lateness the jitter estimate can
/// represent, and so how long a stalled group is waited on before it is
/// skipped, as under `moq play --delay auto`. A shorter wait would cap the
/// estimate at it, since a skipped group is never measured.
pub(crate) const MAX_DELAY: Duration = Duration::from_secs(2);

/// What a bus channel holds between a write and the mix that plays it, while
/// it is kept fed.
///
/// moq-audio's `bus::LATENCY`, which is private; `a_write_sounds_when_placed`
/// pins the two together.
pub(crate) const BUS_LATENCY: Duration = Duration::from_millis(50);

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
			None => now + target + BUS_LATENCY,
			Some((sounds, media)) => {
				let follows = sounds + start.saturating_sub(media);
				// A write half the bus's latency late still finds sound queued
				// ahead of it, however coarsely the bus advances.
				if follows < now + BUS_LATENCY / 2 {
					self.clock.audio_restarted();
					now + target + BUS_LATENCY
				} else if start.saturating_add(CONTIGUOUS) < media
					|| follows.saturating_duration_since(now) > target + BUS_LATENCY + SLACK
				{
					// Media overlapping what is queued, or past the target, would
					// queue without bound however fast it arrives.
					self.tail = Some((sounds, media.max(end)));
					return Place::Skip;
				} else {
					follows
				}
			}
		};
		let sounds = begins + duration;
		self.tail = Some((sounds, end));
		let audio = timeline::Audio {
			end,
			buffered: sounds.saturating_duration_since(now),
			target,
		};
		self.clock.audio(audio, now);
		self.changed.notify_one();
		Place::Play(begins)
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

/// Writes placed frames into a bus channel, so each sounds when it was placed.
///
/// A channel plays a write [`BUS_LATENCY`] later only while it holds that much:
/// one that ran dry plays the next write at once. So the writer keeps track of
/// when what it wrote runs out, and puts silence ahead of a frame for however
/// much of the latency has drained.
pub(crate) struct Writer {
	channel: bus::Channel,
	/// Bytes per sample frame of the channel's `f32` input.
	stride: usize,
	rate: u32,
	/// When what was written finishes sounding, or `None` before the first
	/// write, which the channel's own priming holds back the full latency.
	runs_out: Option<Instant>,
	/// Silence and the frame after it, written as one, so the channel pads
	/// behind the frame rather than between them.
	pending: Vec<u8>,
}

impl Writer {
	/// Write into a new `channel`, taking interleaved `f32` at `rate` in `channels`.
	pub(crate) fn new(channel: bus::Channel, rate: u32, channels: u32) -> Self {
		Self {
			channel,
			stride: channels as usize * size_of::<f32>(),
			rate,
			runs_out: None,
			pending: Vec::new(),
		}
	}

	/// Write `data` at `now` to sound at `sounds`, or as soon after as it can.
	/// Call it no sooner than [`BUS_LATENCY`] ahead.
	pub(crate) fn write(&mut self, sounds: Instant, data: &[u8], now: Instant) -> Result<(), moq_audio::Error> {
		let free = self.runs_out.unwrap_or(now + BUS_LATENCY).max(now);
		let gap = sounds.saturating_duration_since(free).min(BUS_LATENCY);
		let gap = self.frames(gap);
		self.pending.clear();
		self.pending.resize(gap * self.stride, 0);
		self.pending.extend_from_slice(data);
		self.channel.write(&self.pending)?;
		let runs_out = free + self.duration(self.pending.len() / self.stride);
		// A channel left holding a quarter of its latency or less pads itself
		// back up to it behind the write, as moq-audio's ring does.
		self.runs_out = Some(match runs_out.saturating_duration_since(now) <= BUS_LATENCY / 4 {
			true => now + BUS_LATENCY,
			false => runs_out,
		});
		Ok(())
	}

	fn frames(&self, duration: Duration) -> usize {
		(duration.as_secs_f64() * f64::from(self.rate)).round() as usize
	}

	fn duration(&self, frames: usize) -> Duration {
		Duration::from_secs_f64(frames as f64 / f64::from(self.rate))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

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
		assert_eq!(speaker.place(ms(2_080), FRAME, TARGET, at(60)), Place::Skip);
		assert_eq!(speaker.place(ms(2_100), FRAME, TARGET, at(61)), Place::Play(at(230)));
		assert_eq!(clock.due(stamp(2_100)), Some(at(230)));
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
		assert_eq!(speaker.place(ms(1_200), FRAME, TARGET, at(20)), Place::Play(at(190)));
		assert_eq!(clock.due(stamp(1_200)), Some(at(190)));
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
					assert!(begins <= at(0) + TARGET + BUS_LATENCY + SLACK, "{index}: {begins:?}");
				}
			}
		}
		assert_eq!(speaker.place(ms(500), FRAME, TARGET, at(1_000)), Place::Play(at(1_150)));
		assert_eq!(clock.due(stamp(500)), Some(at(1_150)));
	}

	/// A frame sounds when placed: the first, one that follows it, and ones
	/// written after the channel ran dry, on time or late, which the bus would
	/// otherwise play at once or pad behind, apart from their pictures.
	#[test]
	fn a_write_sounds_when_placed() {
		let (bus, mut driver) = moq_audio::bus::Bus::new(Default::default()).unwrap();
		let config = bus.config().clone();
		let rate = u64::from(config.sample_rate);
		let strip = moq_audio::bus::Strip::default();
		let mut input = moq_audio::encode::Input::new(config.sample_rate, config.layout);
		input.format = moq_audio::Format::F32;
		let channel = bus.channel(input, strip.clone()).unwrap();
		let mut writer = Writer::new(channel, config.sample_rate, config.layout.channels());
		let mix = bus.mix();
		mix.route(&strip);
		let mut reader = mix.reader();

		// The bus clock, as an instant, and a frame of `value` on it.
		let block = rate / 100;
		let clock = |frames: u64| Instant::ZERO + Duration::from_micros(frames * 1_000_000 / rate);
		let frame = |value: f32| bytemuck::cast_slice::<f32, u8>(&vec![value; 2 * 960]).to_vec();
		let mut heard = Vec::new();
		let mut position = 0;
		let mut advance = |until: u64, heard: &mut Vec<(u64, f32)>| {
			while position < until {
				driver.advance(block as usize);
				position += block;
				let std::task::Poll::Ready(Some(read)) = reader.poll_read(&kio::Waiter::noop()) else {
					panic!("an advance publishes a block");
				};
				let mut last = heard.last().map_or(0.0, |(_, value)| *value);
				for (index, sample) in read.data.chunks(2).enumerate() {
					if sample[0] != last {
						last = sample[0];
						heard.push((read.position + index as u64, last));
					}
				}
			}
		};

		advance(block, &mut heard);
		let sounds = clock(block) + BUS_LATENCY;
		writer.write(sounds, &frame(1.0), clock(block)).unwrap();
		// The next one, on time.
		advance(block + 960, &mut heard);
		writer.write(sounds + FRAME, &frame(2.0), clock(block + 960)).unwrap();
		// Long after both ran out.
		advance(rate, &mut heard);
		writer
			.write(clock(rate) + BUS_LATENCY, &frame(3.0), clock(rate))
			.unwrap();
		advance(2 * rate, &mut heard);
		// Run dry and written late, with less than the latency to go.
		advance(2 * rate + 1_920, &mut heard);
		writer
			.write(clock(2 * rate) + BUS_LATENCY, &frame(4.0), clock(2 * rate + 1_920))
			.unwrap();
		advance(3 * rate, &mut heard);

		let latency = rate * BUS_LATENCY.as_millis() as u64 / 1_000;
		let first = block + latency;
		let after = rate + latency;
		let late = 2 * rate + latency;
		assert_eq!(
			heard,
			[
				(first, 1.0),
				(first + 960, 2.0),
				(first + 1_920, 0.0),
				(after, 3.0),
				(after + 960, 0.0),
				(late, 4.0),
				(late + 960, 0.0)
			]
		);
	}
}
