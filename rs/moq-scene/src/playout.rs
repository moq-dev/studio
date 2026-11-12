//! When a broadcast source's audio sounds and its pictures show: on its
//! publisher's timestamps, against one [`sync`] clock per broadcast, as in
//! `moq play`.
//!
//! Each track folds its arrivals into the clock, which plays the largest
//! jitter target any track holds behind the earliest of them. In the steady
//! state, sound and pictures stamped alike are due together. A track that
//! stalls only arrives late, and never holds or moves the other: the picture
//! plays on through a sound stall, and the sound comes back on the clock, the
//! part of it already due trimmed. Without audio, video plays [`VIDEO_DELAY`]
//! behind its arrivals.
//!
//! A [`Writer`] hands each sound to the bus [`WRITE_LEAD`] ahead of its
//! moment, at the bus position that moment maps to. A media or replay source's
//! sound goes through a [`Writer`] too, so it sounds with the pictures it
//! shows.

use std::time::Duration;

use kio::clock::Instant;
use moq_audio::bus;
use moq_mux::sync;
use tokio::sync::watch;

use crate::source::Mixed;

/// How far a broadcast without audio plays behind its arrivals, as under
/// `moq play --delay auto`. With audio, video follows the audio's target.
pub(crate) const VIDEO_DELAY: Duration = Duration::from_millis(100);

/// The longest playout delay, the most lateness the jitter estimate can
/// represent, and so how long a stalled group is waited on before it is
/// skipped, as under `moq play --delay auto`. A shorter wait would cap the
/// estimate at it, since a skipped group is never measured.
pub const MAX_DELAY: Duration = Duration::from_secs(2);

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

/// How far apart two stamps may be and still meet: a millisecond-stamped
/// ingest rounds each one.
const CONTIGUOUS: Duration = Duration::from_millis(1);

/// One track's hold on its broadcast's clock: its arrivals, and for audio its
/// jitter target. Every move of the schedule is sent on `moved`, since a track
/// asleep on a deadline computed before it is due earlier or later now.
///
/// Dropping it releases its hold, which may move the schedule too.
pub(crate) struct Hold<'a> {
	track: sync::Track,
	moved: &'a watch::Sender<()>,
}

impl<'a> Hold<'a> {
	/// Hold `clock` for a track of `kind`, the only one of that kind on it.
	pub(crate) fn new(clock: &sync::Producer, kind: moq_mux::container::Kind, moved: &'a watch::Sender<()>) -> Self {
		Self {
			track: clock.track(kind),
			moved,
		}
	}

	/// Fold a frame stamped `timestamp` that arrived at `now` into the clock.
	pub(crate) fn received(&self, timestamp: moq_net::Timestamp, now: Instant) {
		if self.track.received(timestamp, now) {
			self.moved.send_replace(());
		}
	}

	/// Hold the audio's jitter `target`, plus the lead its writes need so a
	/// frame on time is handed to the bus whole, up to [`MAX_DELAY`].
	pub(crate) fn set_target(&self, target: Duration) {
		if self.track.set_target(target.saturating_add(WRITE_LEAD).min(MAX_DELAY)) {
			self.moved.send_replace(());
		}
	}

	/// End this track's timeline at lost media or an encoder restart: its
	/// arrivals hold the clock again from the next frame.
	pub(crate) fn reset(&self) {
		self.track.reset();
		self.moved.send_replace(());
	}
}

impl Drop for Hold<'_> {
	fn drop(&mut self) {
		self.moved.send_replace(());
	}
}

/// How soon a sound waiting on another broadcast's clock looks at it again:
/// that clock moves without telling anyone reading it.
pub(crate) const RECHECK: Duration = Duration::from_millis(10);

/// What to do with a sound at a moment.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Follow {
	/// Write it to sound at this moment.
	Write(Instant),
	/// Look again at this moment, its lead before it sounds.
	Wait(Instant),
	/// It has sounded through, or the clock has no moment for it.
	Drop,
}

/// What to do at `now` with a sound lasting `duration` that the clock says
/// sounds at `due`. Read `due` from the clock as it is now, not as it was when
/// the sound arrived, so a move of the schedule moves the sound. One already
/// partly due is still written: the [`Writer`] trims what the bus has mixed.
pub(crate) fn follow(due: Option<Instant>, duration: Duration, now: Instant) -> Follow {
	let Some(sounds) = due else {
		return Follow::Drop;
	};
	if sounds + duration <= now {
		return Follow::Drop;
	}
	let write = sounds - WRITE_LEAD;
	if write > now {
		return Follow::Wait(write);
	}
	Follow::Write(sounds)
}

/// Writes placed frames into a positioned bus channel, each at the bus
/// position its moment maps to, so it sounds exactly then.
///
/// The bus's [`Mixer`](crate::source::Mixer) advances it sample-exact on the
/// clock from its start, so a moment `t` is bus position `(t - start) * rate`.
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
	/// continues it. The part the bus has already mixed, or overlapping the
	/// last frame, is cut, so the rest still sounds on schedule. The part past
	/// what the bus holds waits until the bus has mixed far enough to hold it,
	/// however far behind the clock it runs. Once the bus's mixer is gone,
	/// nothing mixes, and what is left is dropped.
	pub(crate) async fn write(&mut self, sounds: Instant, data: &[u8]) -> Result<(), moq_audio::Error> {
		let mut position = self.position(sounds);
		if position.abs_diff(self.end) <= self.frames(CONTIGUOUS) {
			position = self.end;
		}
		let end = position + (data.len() / self.stride) as i128;
		let reach = self.frames(bus::REACH) as i128;
		loop {
			// The bus cannot advance while its position is borrowed, so what
			// is written lands where it has not mixed.
			let next = self.mixed.position.borrow_and_update();
			let from = position.max(self.end).max(*next as i128);
			if from >= end {
				return Ok(());
			}
			let until = end.min(*next as i128 + reach);
			if from < until {
				let bytes = |at: i128| (at - position) as usize * self.stride;
				self.channel.write(from as u64, &data[bytes(from)..bytes(until)])?;
				self.end = until;
			}
			drop(next);
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

	/// A sound is written its lead before its moment, and still once partly
	/// due, for the writer to trim; once it has sounded through, or before the
	/// clock has a moment for it, it is dropped.
	#[test]
	fn a_sound_is_written_its_lead_ahead() {
		assert_eq!(follow(Some(at(150)), FRAME, at(0)), Follow::Wait(at(100)));
		assert_eq!(follow(Some(at(150)), FRAME, at(100)), Follow::Write(at(150)));
		assert_eq!(follow(Some(at(150)), FRAME, at(160)), Follow::Write(at(150)));
		assert_eq!(follow(Some(at(150)), FRAME, at(170)), Follow::Drop);
		assert_eq!(follow(None, FRAME, at(0)), Follow::Drop);
	}

	/// Sound and pictures stamped alike are due together, the audio's target
	/// and the write lead behind the first arrival. A sound stall leaves the
	/// pictures on their moments, and the backlog that lands late is dropped
	/// up to what is still to sound, which plays on the clock.
	#[test]
	fn a_sound_stall_leaves_the_pictures_on_their_moments() {
		let clock = sync::Producer::new(VIDEO_DELAY);
		let (moved, mut seen) = watch::channel(());
		let video = Hold::new(&clock, moq_mux::container::Kind::Video, &moved);
		let audio = Hold::new(&clock, moq_mux::container::Kind::Audio, &moved);
		audio.received(stamp(1_000), at(0));
		audio.set_target(TARGET);
		video.received(stamp(1_000), at(0));
		assert!(seen.has_changed().unwrap(), "the first arrival anchors the clock");
		seen.mark_unchanged();
		assert_eq!(clock.due(stamp(1_000)), Some(at(150)));
		assert_eq!(clock.consume().delay(), TARGET + WRITE_LEAD);

		// The audio stalls; the pictures go on arriving, and showing, on time.
		for index in 1..25 {
			video.received(stamp(1_000 + index * 20), at(index * 20));
			assert_eq!(clock.due(stamp(1_000 + index * 20)), Some(at(150 + index * 20)));
		}
		// Its backlog lands half a second late, moving nothing: what sounded
		// through meanwhile is dropped, and the rest written on the clock.
		for index in 0..25 {
			audio.received(stamp(1_000 + index * 20), at(500));
		}
		assert!(!seen.has_changed().unwrap(), "a late track moved the schedule");
		assert_eq!(follow(clock.due(stamp(1_320)), FRAME, at(500)), Follow::Drop);
		assert_eq!(follow(clock.due(stamp(1_340)), FRAME, at(500)), Follow::Write(at(490)));
		assert_eq!(follow(clock.due(stamp(1_400)), FRAME, at(500)), Follow::Write(at(550)));
		assert_eq!(follow(clock.due(stamp(1_420)), FRAME, at(500)), Follow::Wait(at(520)));

		// A stopped audio track lets the pictures play on video's own delay.
		drop(audio);
		assert!(seen.has_changed().unwrap());
		assert_eq!(clock.consume().delay(), VIDEO_DELAY);
	}

	/// However deep the jitter target, playback stays within the longest delay.
	#[test]
	fn the_delay_never_passes_the_longest() {
		let clock = sync::Producer::new(VIDEO_DELAY);
		let (moved, _) = watch::channel(());
		let audio = Hold::new(&clock, moq_mux::container::Kind::Audio, &moved);
		audio.set_target(MAX_DELAY);
		assert_eq!(clock.consume().delay(), MAX_DELAY);
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
				self.block();
			}
		}

		/// Mix the next block now, whatever the clock says.
		fn block(&mut self) {
			self.mixer.advance(Self::BLOCK as usize);
			self.next += Self::BLOCK;
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

		// The bus mixes the block the clock reaches just as a late frame is
		// handed over: its tail lands after that block.
		ear.until(at(5_990)).await;
		kio::clock::sleep_until(at(5_995)).await;
		ear.block();
		writer.write(at(5_990), &frame(9.0, 20)).await.unwrap();
		ear.until(at(6_100)).await;

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
				(48 * 6_000, 9.0),
				(48 * 6_010, 0.0),
			]
		);
	}
}
