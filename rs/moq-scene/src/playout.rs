//! When a broadcast source's audio sounds and its pictures show: on its
//! publisher's timestamps, so they stay in sync through jitter and stalls.
//!
//! A broadcast's audio and video share one [`timeline`] clock, as they do in
//! `moq play`: audio owns the clock's anchor while it plays, and video shows
//! each picture once the clock says it is due. Without audio, video paces
//! itself [`VIDEO_DELAY`] behind its arrivals.
//!
//! `moq play` holds its jitter buffer in the speaker. The bus holds only
//! [`BUS_LATENCY`] of a source, so a [`Speaker`] holds the rest: each frame
//! waits until it is due, the playout target moq-audio's decoder estimates
//! after it arrives, and is written the bus's latency ahead of when it sounds.

use std::time::Duration;

use kio::clock::Instant;
use moq_mux::timeline;

/// How far a broadcast without audio plays behind its arrivals, as under
/// `moq play --delay auto`. With audio, video follows the audio's target.
pub(crate) const VIDEO_DELAY: Duration = Duration::from_millis(100);

/// The longest playout target, the most lateness the jitter estimate can
/// represent, and so how long a stalled group is waited on before it is
/// skipped, as under `moq play --delay auto`. A shorter wait would cap the
/// estimate at it, since a skipped group is never measured.
pub(crate) const MAX_DELAY: Duration = Duration::from_secs(2);

/// What the bus holds between a channel write and the mix that plays it.
///
/// moq-audio's `bus::LATENCY`, which is private; `a_write_sounds_the_bus_latency_later`
/// pins the two together.
pub(crate) const BUS_LATENCY: Duration = Duration::from_millis(50);

/// How far over its target the queue may run before an arrival is skipped to
/// land back on it, as `moq play`'s speaker allows: one frame of cadence.
const SLACK: Duration = Duration::from_millis(20);

/// How far apart two frames' stamps may be and still continue each other: a
/// millisecond-stamped ingest rounds each one.
const CONTIGUOUS: Duration = Duration::from_millis(1);

/// Places one broadcast audio track's frames on the bus clock, and reports
/// where they sound to the broadcast's clock, which video follows.
///
/// Dropping it hands the clock's anchor back to video.
pub(crate) struct Speaker {
	clock: timeline::Producer,
	/// The last frame placed: when it finishes sounding, and the media time it
	/// ends at.
	tail: Option<(Instant, Duration)>,
}

/// What to do with a frame [`Speaker::place`] was given.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Place {
	/// Write it to the bus at `at`. `moved` says the clock's anchor moved, so
	/// every picture waiting for its moment is due at another one now.
	Write { at: Instant, moved: bool },
	/// Drop it: more is queued than the target needs, after a burst or a
	/// target that fell, so the media skips ahead to land back on the target.
	Skip,
}

impl Speaker {
	/// Place audio on `clock`, taking its anchor from video on the first frame.
	pub(crate) fn new(clock: timeline::Producer) -> Self {
		Self { clock, tail: None }
	}

	/// Place a decoded frame covering `duration` of media from `start`,
	/// arriving at `now`, given the playout `target` moq-audio estimated.
	///
	/// A frame continues the one before, after silence for any hole in the
	/// media between them. A frame that arrives too late for that, because the
	/// network stalled longer than the target, re-buffers instead: it sounds the
	/// whole target after it arrived, and the clock re-pins on it, so video
	/// waits along with the sound rather than running ahead of it.
	pub(crate) fn place(&mut self, start: Duration, duration: Duration, target: Duration, now: Instant) -> Place {
		let target = target.min(MAX_DELAY);
		let end = start.saturating_add(duration);
		// The bus rides out a write up to half its latency late without a dropout.
		let follows = self.tail.and_then(|(sounds, media)| {
			let begins = sounds + start.saturating_sub(media);
			let rewound = start.saturating_add(CONTIGUOUS) < media;
			let dry = begins < now + BUS_LATENCY / 2;
			(!rewound && !dry).then_some((begins, sounds))
		});
		let begins = match follows {
			Some((begins, sounds)) => {
				if begins.saturating_duration_since(now) > target + BUS_LATENCY + SLACK {
					self.tail = Some((sounds, end));
					return Place::Skip;
				}
				begins
			}
			None => {
				let begins = now + target + BUS_LATENCY;
				match self.tail {
					Some((sounds, _)) => {
						self.clock.audio_restarted();
						// Never over audio still queued, after a rewind.
						begins.max(sounds)
					}
					None => begins,
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
		let moved = self.clock.audio(audio, now);
		Place::Write {
			at: begins - BUS_LATENCY,
			moved,
		}
	}
}

impl Drop for Speaker {
	fn drop(&mut self) {
		self.clock.audio_stopped();
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

	fn written(place: Place) -> Instant {
		match place {
			Place::Write { at, .. } => at,
			Place::Skip => panic!("skipped"),
		}
	}

	/// The first frame waits out the target, later ones follow it on their
	/// stamps however unevenly they arrive within it, and a picture is due when
	/// the sound stamped with it plays: the bus's latency after its write.
	#[test]
	fn video_is_due_when_its_audio_sounds() {
		let clock = timeline::Producer::new(VIDEO_DELAY);
		let mut speaker = Speaker::new(clock.clone());
		assert_eq!(written(speaker.place(ms(1_000), FRAME, TARGET, at(0))), at(100));
		assert_eq!(clock.due(stamp(1_000)), Some(at(150)));
		// Late within the target, and early: both keep their place.
		assert_eq!(written(speaker.place(ms(1_020), FRAME, TARGET, at(90))), at(120));
		assert_eq!(written(speaker.place(ms(1_040), FRAME, TARGET, at(91))), at(140));
		assert_eq!(clock.due(stamp(1_040)), Some(at(190)));
		assert_eq!(clock.consume().target(), Some(TARGET));

		// A stopped track hands the clock back to video.
		drop(speaker);
		assert_eq!(clock.consume().target(), None);
	}

	/// A stall longer than the target re-buffers: the next frame sounds the
	/// target after it arrived, and the pictures stamped with it wait as long.
	#[test]
	fn a_stall_holds_video_back_with_the_sound() {
		let clock = timeline::Producer::new(VIDEO_DELAY);
		let mut speaker = Speaker::new(clock.clone());
		speaker.place(ms(1_000), FRAME, TARGET, at(0));
		let place = speaker.place(ms(1_020), FRAME, TARGET, at(500));
		assert!(matches!(place, Place::Write { at: written, moved: true } if written == at(600)));
		assert_eq!(clock.due(stamp(1_020)), Some(at(650)));
	}

	/// Lost media plays as the silence it was, so the pictures after it keep
	/// their moments; a hole past the target skips instead, and the clock
	/// re-pins on the frame after it.
	#[test]
	fn a_hole_plays_as_silence_up_to_the_target() {
		let clock = timeline::Producer::new(VIDEO_DELAY);
		let mut speaker = Speaker::new(clock.clone());
		speaker.place(ms(1_000), FRAME, TARGET, at(0));
		let place = speaker.place(ms(1_060), FRAME, TARGET, at(40));
		assert_eq!(
			place,
			Place::Write {
				at: at(160),
				moved: false
			}
		);

		// 1 s lost, with only the target queued: skipped back onto the target.
		assert_eq!(speaker.place(ms(2_080), FRAME, TARGET, at(60)), Place::Skip);
		let place = speaker.place(ms(2_100), FRAME, TARGET, at(61));
		assert!(matches!(place, Place::Write { at: written, moved: true } if written == at(180)));
		assert_eq!(clock.due(stamp(2_100)), Some(at(230)));
	}

	/// A burst queues no more than the target and a frame of slack: the rest
	/// is skipped, and play goes on from what arrives next.
	#[test]
	fn a_burst_skips_past_the_target() {
		let clock = timeline::Producer::new(VIDEO_DELAY);
		let mut speaker = Speaker::new(clock.clone());
		let placed = (0..10)
			.filter_map(
				|index| match speaker.place(ms(1_000 + index * 20), FRAME, TARGET, at(0)) {
					Place::Write { at, .. } => Some(at),
					Place::Skip => None,
				},
			)
			.collect::<Vec<_>>();
		assert_eq!(placed, [at(100), at(120)]);
		let place = speaker.place(ms(1_200), FRAME, TARGET, at(20));
		assert!(matches!(place, Place::Write { at: written, moved: true } if written == at(140)));
		assert_eq!(clock.due(stamp(1_200)), Some(at(190)));
	}

	/// The bus plays a channel's first write, and each one after it on time,
	/// [`BUS_LATENCY`] after it was written.
	#[test]
	fn a_write_sounds_the_bus_latency_later() {
		let (bus, mut driver) = moq_audio::bus::Bus::new(Default::default()).unwrap();
		let rate = u64::from(bus.config().sample_rate);
		let strip = moq_audio::bus::Strip::default();
		let mut input = moq_audio::encode::Input::new(bus.config().sample_rate, bus.config().layout);
		input.format = moq_audio::Format::F32;
		let mut channel = bus.channel(input, strip.clone()).unwrap();
		let mix = bus.mix();
		mix.route(&strip);
		let mut reader = mix.reader();

		let block = rate as usize / 100;
		driver.advance(block);
		let written = block as u64;
		let ones = vec![1.0f32; 2 * block];
		channel.write(bytemuck::cast_slice(&ones)).unwrap();

		let sounded = loop {
			driver.advance(block);
			let std::task::Poll::Ready(Some(block)) = reader.poll_read(&kio::Waiter::noop()) else {
				panic!("an advance publishes a block");
			};
			if let Some(index) = block.data.chunks(2).position(|frame| frame[0] != 0.0) {
				break block.position + index as u64;
			}
		};
		assert_eq!(
			Duration::from_micros((sounded - written) * 1_000_000 / rate),
			BUS_LATENCY
		);
	}
}
