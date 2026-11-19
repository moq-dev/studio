//! Instant replay: the recent past of a broadcast, held so it can be played
//! back at any speed or saved.
//!
//! The groups are already on hand. A track keeps its recent groups as far back
//! as its `max_age`, whoever watches, so a [`Take`] copies those covering a span
//! into a broadcast of its own. Nothing ages a take's groups, so a slow-motion
//! replay that ends long after its span left the track's window still has all
//! of it, and its tracks end where the span does, so an export of it finishes.
//! Copying shares each frame's payload rather than duplicating it.

use std::collections::BTreeMap;
use std::task::Poll;
use std::time::Duration;

use hang::catalog::{AudioConfig, Reference, VideoConfig};

use crate::Error;

/// The longest span a take holds: as far back as a media track keeps its groups.
///
/// Further back, a track holds nothing to copy, and a relay asked for it would
/// fetch it from an archive, so bounding the span bounds what a take can make
/// the studio hold to that many seconds of the broadcast.
pub const MAX_LENGTH: Duration = hang::container::MAX_AGE;

/// The slowest a take plays, as a fraction of real time.
///
/// A replay holds its take until it ends, so this bounds that to
/// [`MAX_LENGTH`] / `MIN_SPEED`, and keeps when each moment plays well within
/// the clock's range.
pub const MIN_SPEED: f32 = 0.01;

/// What a take copies: a broadcast's video and audio renditions, and the
/// moment its span ends, on that broadcast's timeline.
#[derive(Clone)]
pub struct Target {
	/// The broadcast whose tracks are copied.
	pub broadcast: moq_net::broadcast::Consumer,
	/// The video rendition, by catalog name.
	pub video: Option<(String, VideoConfig)>,
	/// The audio rendition, by catalog name.
	pub audio: Option<(String, AudioConfig)>,
	/// The newest moment the span covers.
	pub end: moq_net::Timestamp,
}

/// A span of a broadcast, copied into a broadcast of its own.
///
/// It holds the groups covering the span, from the one in progress at its start
/// through the one in progress at its end, so the span decodes from its first
/// picture. Dropping it releases them.
///
/// Frames are in decode order, so with B-frames a picture shown inside the span
/// can refer to one shown after its end. A take to play ([`Take::new`]) keeps
/// such references, since playing stops showing at the end anyway. A clip
/// ([`Take::clip`]) stops before the first frame shown after the end, since an
/// export shows everything it holds. It loses every frame decoded after that
/// one too, so it can end early by as much as the span that frame's group
/// reorders, back to the group's keyframe at worst.
pub struct Take {
	broadcast: moq_net::broadcast::Producer,
	/// Keep the copies and their listing readable.
	_tracks: Vec<moq_net::track::Finished>,
	_listing: moq_mux::catalog::Producer,
	/// The video rendition, under its catalog name.
	pub video: Option<(String, VideoConfig)>,
	/// The audio rendition, under its catalog name.
	pub audio: Option<(String, AudioConfig)>,
	/// How much of the span the video's pictures cover: less where the
	/// rendition was not encoding, gaps inside the span included, but not for
	/// pictures it dropped in between.
	pub covered: Duration,
	/// Where the span starts on the copied broadcast's timeline.
	pub start: moq_net::Timestamp,
	/// Where it ends.
	pub end: moq_net::Timestamp,
}

impl Take {
	/// Hold the `length` of `target` that ends at its end, at most [`MAX_LENGTH`],
	/// as far as its tracks hold it now.
	///
	/// Copies only what is readable at once, so it never waits on a publisher:
	/// a rendition its publisher has not described yet, or a frame or group
	/// still to be written or still in transit, is left out, and a span ending
	/// at the live edge may stop a frame or two short.
	///
	/// Refuses a target with neither rendition, one served from another
	/// broadcast, a length of zero or over [`MAX_LENGTH`], a broadcast that holds
	/// nothing of the span, such as one that has described neither rendition
	/// yet or encoded nothing then, and, over a wire that cannot address a
	/// received track by time, a broadcast from a relay.
	pub fn new(target: &Target, length: Duration) -> Result<Self, Error> {
		Self::cut(target, length, Cut::Decodable)
	}

	/// Hold the `length` of `target` that ends at its end to save, as
	/// [`Take::new`] does, but with nothing shown after the end.
	pub fn clip(target: &Target, length: Duration) -> Result<Self, Error> {
		Self::cut(target, length, Cut::Shown)
	}

	fn cut(target: &Target, length: Duration, cut: Cut) -> Result<Self, Error> {
		if length.is_zero() || length > MAX_LENGTH {
			return Err(Error::Invalid(format!(
				"a replay is above 0 and at most {} s long, not {length:?}",
				MAX_LENGTH.as_secs()
			)));
		}
		if target.video.is_none() && target.audio.is_none() {
			return Err(Error::Unsupported(
				"the broadcast has no video or audio to replay".into(),
			));
		}
		let end = target.end;
		let start = Duration::try_from(end)
			.map(|end| end.saturating_sub(length))
			.and_then(moq_net::Timestamp::try_from)
			.and_then(|start| start.convert(end.scale().ok_or(moq_net::TimeOverflow)?))
			.map_err(|err| Error::Invalid(format!("replay start: {err}")))?;

		let mut broadcast = moq_net::broadcast::Info::new().produce();
		let mut tracks = Vec::new();
		let mut video = None;
		let mut covered = Duration::ZERO;
		if let Some((name, config)) = &target.video {
			reference(config)?;
			let container = moq_mux::catalog::hang::Container::try_from(config)?;
			let track = Reference::track(config, name);
			if let Some((track, held)) = copy(target, &broadcast, track, start, cut, &container)? {
				tracks.push(track);
				covered = held;
				// What the take holds plays, even from a rendition the
				// publisher has since stopped encoding.
				let mut config = config.clone();
				config.enabled = true;
				video = Some((name.clone(), config));
			}
		}
		let mut audio = None;
		if let Some((name, config)) = &target.audio {
			reference(config)?;
			let container = moq_mux::catalog::hang::Container::try_from(config)?;
			let track = Reference::track(config, name);
			if let Some((track, _)) = copy(target, &broadcast, track, start, cut, &container)? {
				tracks.push(track);
				let mut config = config.clone();
				config.enabled = true;
				audio = Some((name.clone(), config));
			}
		}
		if tracks.is_empty() {
			return Err(Error::Unsupported(format!(
				"nothing of the last {} s is held: the broadcast encoded nothing then, or has not described its \
				 tracks yet",
				length.as_secs_f32()
			)));
		}
		// Listed so an export of the take finds its renditions.
		let mut listing = moq_mux::catalog::Producer::new(&mut broadcast, Default::default())?;
		listing.mutate(|listed| {
			listed.video.renditions = video.clone().into_iter().collect();
			listed.audio.renditions = audio.clone().into_iter().collect();
		})?;
		listing.finish()?;

		Ok(Self {
			broadcast,
			_tracks: tracks,
			_listing: listing,
			video,
			audio,
			covered,
			start,
			end,
		})
	}

	/// The take's broadcast, to decode or export.
	pub fn consume(&self) -> moq_net::broadcast::Consumer {
		self.broadcast.consume()
	}
}

/// Refuse a rendition served from another broadcast, which the take does not
/// subscribe to.
fn reference(config: &impl Reference) -> Result<(), Error> {
	match config.broadcast() {
		Some(_) => Err(Error::Unsupported(
			"a rendition served from another broadcast cannot be replayed".into(),
		)),
		None => Ok(()),
	}
}

/// Where a take's groups stop in decode order, at the end of its span.
#[derive(Clone, Copy)]
enum Cut {
	/// Through the last frame shown inside the span, with every reference it
	/// needs, even one shown after the end.
	Decodable,
	/// Before the first frame shown after the end.
	Shown,
}

/// Copy the span of `name` in `target`, as far as it is readable now, into a
/// track of `broadcast`, and end that track, with how much of the span its
/// media covers. `None` when it holds no media of the span, only the markers
/// of a publisher's gaps if anything, or while its publisher has not
/// described the track. `container` reads its frames.
fn copy(
	target: &Target,
	broadcast: &moq_net::broadcast::Producer,
	name: &str,
	start: moq_net::Timestamp,
	cut: Cut,
	container: &impl moq_mux::container::Container,
) -> Result<Option<(moq_net::track::Finished, Duration)>, Error> {
	// The anchors are group starts, in the timescale of hang's media tracks.
	let key = |at: moq_net::Timestamp| {
		at.convert(hang::container::TIMESCALE)
			.map(moq_net::Timestamp::value)
			.map_err(|err| Error::Invalid(format!("replay span: {err}")))
	};
	let range = moq_net::track::Range {
		start: moq_net::track::Anchor::Time(key(start)?),
		end: Some(moq_net::track::Anchor::Time(key(target.end)?)),
	};
	// With the publisher's whole window, so none of the span is skipped as
	// stale.
	let subscription = moq_net::track::Subscription::default()
		.with_ranges([range])
		.with_delay(moq_net::track::Delay::default().with_max(None))
		.with_order(moq_net::track::Order::Ascending);
	let subscribing = target.broadcast.track(name)?.subscribe(subscription);
	// Never woken, like the reads below: a publisher that never describes the
	// track cannot hold the take up.
	let mut source = match subscribing.poll_ok(&kio::Waiter::noop()) {
		Poll::Ready(source) => source?,
		Poll::Pending => return Ok(None),
	};
	// The range was keyed before the track described itself.
	if source.info().timescale != Some(hang::container::TIMESCALE) {
		return Err(Error::Unsupported(format!(
			"replaying {name}, which is not on hang's media timescale"
		)));
	}
	// Never stale: the take keeps its groups for as long as it lives.
	let info = source.info().clone().with_max_age(None);
	let mut track = broadcast.create_track(name, info)?;
	let covered = match copy_held(&mut source, &mut track, start..=target.end, cut, container) {
		Ok(Some(covered)) => covered,
		Ok(None) => {
			let _ = track.abort(moq_net::Error::Cancel);
			return Ok(None);
		}
		Err(err) => {
			let _ = track.clone().abort(moq_net::Error::Cancel);
			return Err(err);
		}
	};
	Ok(Some((track.finish()?, covered)))
}

/// Whether `frame` is only the marker a publisher writes at a gap: one media
/// frame with nothing in it, as `container` reads it. A frame it cannot read
/// counts as media, since only the publisher's marker is known to be empty.
fn marker(frame: &moq_net::frame::Frame, container: &impl moq_mux::container::Container) -> bool {
	match container.decode(frame.clone()) {
		Ok(frames) => !frames.is_empty() && frames.iter().all(|frame| frame.payload.is_empty()),
		Err(_) => false,
	}
}

/// A group's frames as far as they were readable, and how reading it ended.
struct Held {
	frames: Vec<moq_net::frame::Frame>,
	read: Result<(), moq_net::Error>,
	/// Where the source ended the group, for one held whole.
	end: Option<u64>,
}

/// Copy every group and frame `source` has ready, up to `end` as `cut` says,
/// into `track`, returning how much of `span` its media covers, or
/// `None` without media anywhere.
fn copy_held(
	source: &mut moq_net::track::Subscriber,
	track: &mut moq_net::track::Producer,
	span: std::ops::RangeInclusive<moq_net::Timestamp>,
	cut: Cut,
	container: &impl moq_mux::container::Container,
) -> Result<Option<Duration>, Error> {
	let end = *span.end();
	// Never woken: anything not ready now is left out.
	let now = kio::Waiter::noop();
	// Groups come in the order they reached the studio, which over a session
	// need not be their own.
	let mut held = BTreeMap::new();
	loop {
		let mut group = match source.poll_recv_group(&now) {
			Poll::Ready(group) => match group? {
				Some(group) => group,
				None => break,
			},
			Poll::Pending => break,
		};
		let mut frames = Vec::new();
		let read = loop {
			match group.poll_read_frame(&now) {
				Poll::Ready(Ok(Some(frame))) => frames.push(frame),
				// A group still arriving is copied as far as it arrived.
				Poll::Ready(Ok(None)) | Poll::Pending => break Ok(()),
				Poll::Ready(Err(err)) => break Err(err),
			}
		};
		// A frame without a time cannot be past the end.
		let inside = |frame: &moq_net::frame::Frame| frame.timestamp.scale().is_none() || frame.timestamp <= end;
		let kept = match cut {
			Cut::Decodable => frames.iter().rposition(inside).map_or(0, |last| last + 1),
			Cut::Shown => frames.iter().position(|frame| !inside(frame)).unwrap_or(frames.len()),
		};
		// None kept is a group wholly past the span, or one the publisher gave
		// up or that aged out before any of it was read, which replays as the
		// gap it is.
		if kept > 0 {
			// A group cut short ends where its pictures do, which only a reader
			// can tell.
			let whole = kept == frames.len();
			frames.truncate(kept);
			let end = group.end().filter(|_| whole);
			held.insert(group.start, Held { frames, read, end });
		}
	}

	if let Some(&first) = held.keys().next() {
		track.start_at(first)?;
	}
	for (&start, held) in &held {
		let mut into = track.create_group(moq_net::group::Info { start })?;
		for frame in &held.frames {
			into.write_frame(frame.timestamp, frame.payload.clone())?;
		}
		match (&held.read, held.end) {
			(Ok(()), Some(end)) => drop(into.finish_at(end)?),
			(Ok(()), None) => drop(into.finish()?),
			(Err(err), _) => into.abort(err.clone())?,
		}
	}

	// The time inside the span the media covers, from each picture to the
	// next. A gap's marker or a missing group breaks it, so the time a
	// rendition was not encoding counts for nothing, while a rendition that
	// dropped pictures still covers the time between those it kept. Its media
	// may all be from before the span, as a clip cut back to its keyframe is.
	let mut media = false;
	let mut covered = Duration::ZERO;
	let mut last: Option<(u64, Duration)> = None;
	for (&start, held) in &held {
		// Groups follow on only when the publisher said nothing lies between them.
		last = last.filter(|(previous, _)| source.is_skipped(previous + 1..start));
		for frame in &held.frames {
			if marker(frame, container) {
				last = None;
				continue;
			}
			media = true;
			let Some(at) = Some(frame.timestamp)
				.filter(|at| span.contains(at))
				.and_then(|at| Duration::try_from(at).ok())
			else {
				continue;
			};
			// Decode order can show a picture before the one decoded ahead of
			// it, so only time past the newest shown counts.
			last = match last {
				Some((_, newest)) if at <= newest => Some((start, newest)),
				Some((_, newest)) => {
					covered += at - newest;
					Some((start, at))
				}
				None => Some((start, at)),
			};
		}
	}

	// Carry over only the starts the source declared skipped, before its
	// first group too, which marks the encoder's first. A group lost on the way
	// stays a hole, which the finished take lets readers pass as the gap it
	// is: a skip would tell them the media runs on across it.
	let copied: Vec<u64> = held.into_keys().collect();
	let mut below = 0;
	for &start in &copied {
		if below < start && source.is_skipped(below..start) {
			track.skip_groups(below..start);
		}
		below = start + 1;
	}
	Ok(media.then_some(covered))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn ts(millis: u64) -> moq_net::Timestamp {
		moq_net::Timestamp::from_micros(millis * 1000).unwrap()
	}

	/// Write group `sequence` of `track` with a frame at each of `at`, in
	/// milliseconds and decode order, the frame at `at[i]` named
	/// `{sequence}.{i}`. The group starts at its first frame, in microseconds
	/// as hang's tracks count, and says nothing of the groups around it.
	fn write(track: &moq_net::track::Producer, sequence: u64, at: &[u64]) {
		fill(
			track
				.create_group(moq_net::group::Info { start: at[0] * 1000 })
				.unwrap(),
			sequence,
			at,
		);
	}

	/// [`write`] as a publisher appends, saying nothing lies between this group
	/// and the one before.
	fn append(track: &moq_net::track::Producer, sequence: u64, at: &[u64]) {
		fill(track.append_group(ts(at[0])).unwrap(), sequence, at);
	}

	fn fill(mut group: moq_net::group::Producer, sequence: u64, at: &[u64]) {
		for (index, at) in at.iter().enumerate() {
			group
				.write_frame(ts(*at), format!("{sequence}.{index}").into_bytes())
				.unwrap();
		}
		group.finish().unwrap();
	}

	/// A group's second, from its start in microseconds.
	fn second_of(start: u64) -> u64 {
		start / 1_000_000
	}

	/// The frames of group `sequence` of [`broadcast`], three a third of a
	/// second apart from its second.
	fn second(sequence: u64) -> [u64; 3] {
		[0, 333, 666].map(|at| sequence * 1000 + at)
	}

	/// A video track with a group a second, from `0` up to but excluding
	/// `groups` seconds, but for the group lost on the way at `lost`. The
	/// publisher appends each group, saying nothing lies between them, up to the
	/// loss; past it, nothing says what lies before a group, as a copy that lost
	/// one knows.
	fn broadcast(groups: u64, lost: Option<u64>) -> (moq_net::broadcast::Producer, moq_net::track::Producer) {
		let broadcast = moq_net::broadcast::Info::new().produce();
		let track = broadcast
			.create_track("video", hang::container::track_info(hang::catalog::PRIORITY.video))
			.unwrap();
		for sequence in 0..groups {
			match lost {
				Some(lost) if sequence == lost => {}
				Some(lost) if sequence > lost => write(&track, sequence, &second(sequence)),
				_ => append(&track, sequence, &second(sequence)),
			}
		}
		(broadcast, track)
	}

	/// Whether the take's track declares every group start in `starts`, in
	/// microseconds, skipped.
	async fn skipped(take: &Take, starts: std::ops::Range<u64>) -> bool {
		let track = take.consume().track("video").unwrap().subscribe(None).await.unwrap();
		track.is_skipped(starts)
	}

	fn target(broadcast: &moq_net::broadcast::Producer, end: moq_net::Timestamp) -> Target {
		let config = hang::catalog::VideoConfig::new(hang::catalog::H264 {
			inline: true,
			profile: 0x42,
			constraints: 0,
			level: 0x1f,
		});
		Target {
			broadcast: broadcast.consume(),
			video: Some(("video".into(), config)),
			audio: None,
			end,
		}
	}

	/// Every frame of every group the take's track holds, by group.
	async fn read(take: &Take) -> Vec<(u64, Vec<String>)> {
		let mut track = take
			.consume()
			.track("video")
			.unwrap()
			.subscribe(
				moq_net::track::Subscription::default().with_delay(moq_net::track::Delay::default().with_max(None)),
			)
			.await
			.unwrap();
		let mut groups = Vec::new();
		while let Some(mut group) = track.recv_group().await.unwrap() {
			let mut frames = Vec::new();
			while let Some(frame) = group.read_frame().await.unwrap() {
				frames.push(String::from_utf8(frame.payload.to_vec()).unwrap());
			}
			groups.push((second_of(group.start), frames));
		}
		groups
	}

	fn frames(group: u64, count: u64) -> Vec<String> {
		(0..count).map(|frame| format!("{group}.{frame}")).collect()
	}

	/// A take holds the group in progress at its start through the frames up to
	/// its end, and ends there.
	#[tokio::test(start_paused = true)]
	async fn a_take_holds_its_span() {
		let (broadcast, _track) = broadcast(10, None);
		let mut target = target(&broadcast, ts(7_500));
		// Disabled since, such as for lack of bandwidth, which leaves what it
		// held playable.
		target.video.as_mut().unwrap().1.enabled = false;
		let take = Take::new(&target, Duration::from_secs(3)).unwrap();
		assert!(take.video.as_ref().unwrap().1.enabled);
		assert_eq!(take.start, ts(4_500));
		assert_eq!(
			read(&take).await,
			[
				(4, frames(4, 3)),
				(5, frames(5, 3)),
				(6, frames(6, 3)),
				(7, frames(7, 2))
			]
		);
	}

	/// A group missing from the span stays missing, rather than one its readers
	/// wait for, and a gap rather than a span the media runs on across.
	#[tokio::test(start_paused = true)]
	async fn a_take_keeps_a_gap() {
		let (broadcast, _track) = broadcast(10, Some(5));
		let take = Take::new(&target(&broadcast, ts(7_500)), Duration::from_secs(3)).unwrap();
		assert_eq!(
			read(&take).await,
			[(4, frames(4, 3)), (6, frames(6, 3)), (7, frames(7, 2))]
		);
		assert!(!skipped(&take, 4_000_001..6_000_000).await, "the lost group is a gap");
	}

	/// A group held whole ends where its source ended it, and one cut at the
	/// span's end only closes.
	#[tokio::test(start_paused = true)]
	async fn a_take_keeps_where_its_whole_groups_end() {
		let (broadcast, track) = broadcast(0, None);
		for sequence in 0..3 {
			let mut group = track.append_group(ts(sequence * 1_000)).unwrap();
			for (index, at) in second(sequence).iter().enumerate() {
				group
					.write_frame(ts(*at), format!("{sequence}.{index}").into_bytes())
					.unwrap();
			}
			drop(group.finish_at((sequence + 1) * 1_000_000).unwrap());
		}
		let take = Take::new(&target(&broadcast, ts(2_500)), Duration::from_secs(2)).unwrap();
		let mut copied = take
			.consume()
			.track("video")
			.unwrap()
			.subscribe(
				moq_net::track::Subscription::default().with_delay(moq_net::track::Delay::default().with_max(None)),
			)
			.await
			.unwrap();
		let mut ends = Vec::new();
		while let Some(mut group) = copied.recv_group().await.unwrap() {
			while group.read_frame().await.unwrap().is_some() {}
			ends.push((second_of(group.start), group.end()));
		}
		assert_eq!(ends, [(0, Some(1_000_000)), (1, Some(2_000_000)), (2, None)]);
	}

	/// A take says what the source said of the starts it skipped: between its
	/// groups, and before the source's first group, which marks the encoder's
	/// first, but not before a take that starts later.
	#[tokio::test(start_paused = true)]
	async fn a_take_keeps_the_skips_it_was_given() {
		let (broadcast, track) = broadcast(0, None);
		track.skip_groups(0..1_000_000);
		for sequence in 1..10 {
			append(&track, sequence, &second(sequence));
		}
		let from_first = Take::new(&target(&broadcast, ts(3_500)), Duration::from_secs(3)).unwrap();
		assert_eq!(read(&from_first).await[0].0, 1);
		assert!(skipped(&from_first, 0..1_000_000).await, "the encoder's first group");
		assert!(skipped(&from_first, 1_000_001..2_000_000).await);

		let later = Take::new(&target(&broadcast, ts(7_500)), Duration::from_secs(3)).unwrap();
		assert_eq!(read(&later).await[0].0, 4);
		assert!(!skipped(&later, 0..4_000_000).await, "past the encoder's first");
		assert!(skipped(&later, 4_000_001..5_000_000).await);
	}

	/// Groups that reached the studio out of order are held in their own order,
	/// the first of them included.
	#[tokio::test(start_paused = true)]
	async fn a_take_orders_its_groups() {
		let (broadcast, track) = broadcast(5, None);
		write(&track, 6, &second(6));
		write(&track, 5, &second(5));
		let take = Take::new(&target(&broadcast, ts(6_500)), Duration::from_secs(3)).unwrap();
		assert_eq!(
			read(&take).await,
			[
				(3, frames(3, 3)),
				(4, frames(4, 3)),
				(5, frames(5, 3)),
				(6, frames(6, 2))
			]
		);
	}

	/// A picture presented past the end is kept to play when one decoded after
	/// it, and shown inside the span, may refer to it, and left out of a clip,
	/// with the pictures that refer to it.
	#[tokio::test(start_paused = true)]
	async fn a_take_keeps_the_references_of_its_last_pictures() {
		let (broadcast, track) = broadcast(4, None);
		write(&track, 4, &[4_000, 4_400, 4_133, 4_266]);
		let target = target(&broadcast, ts(4_200));
		let take = Take::new(&target, Duration::from_secs(1)).unwrap();
		assert_eq!(read(&take).await, [(3, frames(3, 3)), (4, frames(4, 3))]);
		let clip = Take::clip(&target, Duration::from_secs(1)).unwrap();
		assert_eq!(read(&clip).await, [(3, frames(3, 3)), (4, frames(4, 1))]);
	}

	/// A clip of a slow source that reorders deeply can end seconds early: at
	/// one picture a second, decoded I0 P8 B1 ... B7, a clip to 7.5 s keeps
	/// only the keyframe, since every picture after it refers to P8.
	#[tokio::test(start_paused = true)]
	async fn a_clip_can_end_back_at_its_keyframe() {
		let (broadcast, track) = broadcast(0, None);
		write(&track, 0, &[0, 8_000, 1_000, 2_000, 3_000, 4_000, 5_000, 6_000, 7_000]);
		let target = target(&broadcast, ts(7_500));
		let clip = Take::clip(&target, Duration::from_secs(5)).unwrap();
		assert_eq!(read(&clip).await, [(0, frames(0, 1))]);
		let take = Take::new(&target, Duration::from_secs(5)).unwrap();
		assert_eq!(read(&take).await, [(0, frames(0, 9))]);
	}

	/// A take at the live edge holds what was written when it was taken, and
	/// ends without waiting for more: a source may never write again, such as a
	/// screen that stopped changing.
	#[tokio::test(start_paused = true)]
	async fn a_take_at_the_live_edge_holds_what_was_written() {
		let (broadcast, track) = broadcast(7, None);
		let mut live = track.append_group(ts(7_000)).unwrap();
		live.write_frame(ts(7_000), b"7.0".to_vec()).unwrap();
		let take = Take::new(&target(&broadcast, ts(7_500)), Duration::from_secs(3)).unwrap();
		live.write_frame(ts(7_333), b"7.1".to_vec()).unwrap();
		assert_eq!(
			read(&take).await,
			[
				(4, frames(4, 3)),
				(5, frames(5, 3)),
				(6, frames(6, 3)),
				(7, frames(7, 1))
			]
		);
	}

	/// A rendition its publisher has not described yet is left out rather than
	/// waited for, and a take of nothing but such renditions is refused.
	/// Outside any runtime, too, as the UI thread takes one.
	#[test]
	fn a_take_leaves_out_what_is_not_described() {
		let (broadcast, _track) = broadcast(10, None);
		// Answers no request, as a publisher that never replies.
		let _requests = broadcast.requests();
		let mut target = target(&broadcast, ts(7_500));
		target.audio = Some((
			"audio".into(),
			hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Opus, 48_000, 2),
		));
		let take = Take::new(&target, Duration::from_secs(1)).unwrap();
		assert!(take.video.is_some());
		assert!(take.audio.is_none());

		target.video = None;
		let result = Take::new(&target, Duration::from_secs(1));
		assert!(matches!(result, Err(Error::Unsupported(_))));
	}

	/// A take says how much of its span holds pictures, and one whose span
	/// holds only the marker of a gap, such as a rendition the grant disabled,
	/// is refused as holding nothing.
	#[test]
	fn a_take_of_a_gap_holds_no_pictures() {
		use moq_mux::container::Container;

		let (broadcast, track) = broadcast(3, None);
		let take = Take::new(&target(&broadcast, ts(2_900)), Duration::from_secs(2)).unwrap();
		// The span starts at 0.900, past group 0's pictures.
		assert_eq!(take.covered, Duration::from_millis(1_666), "from 1.000 to 2.666");

		// The marker a publisher writes as it stops encoding.
		let config = target(&broadcast, ts(0)).video.unwrap().1;
		let container = moq_mux::catalog::hang::Container::try_from(&config).unwrap();
		let mut group = track.append_group(ts(3_500)).unwrap();
		let gap = moq_mux::container::Frame {
			timestamp: ts(3_500),
			payload: bytes::Bytes::new(),
			keyframe: false,
			duration: None,
		};
		container.write(&mut group, &[gap]).unwrap();
		group.finish().unwrap();
		let result = Take::new(&target(&broadcast, ts(3_900)), Duration::from_millis(300));
		assert!(matches!(result, Err(Error::Unsupported(_))), "{:?}", result.err());
	}

	/// Coverage is time, not pictures: a rendition at half the frame rate
	/// throughout covers more than one at the full rate that stopped partway,
	/// and a missing group breaks it.
	#[test]
	fn coverage_counts_time_not_pictures() {
		let take = |seconds: std::ops::Range<u64>, per_second: u64| {
			let broadcast = moq_net::broadcast::Info::new().produce();
			let track = broadcast
				.create_track("video", hang::container::track_info(hang::catalog::PRIORITY.video))
				.unwrap();
			for second in seconds {
				let at: Vec<u64> = (0..per_second)
					.map(|index| second * 1000 + index * 1000 / per_second)
					.collect();
				append(&track, second, &at);
			}
			let take = Take::new(&target(&broadcast, ts(9_999)), Duration::from_secs(10)).unwrap();
			(take.covered, broadcast, track)
		};
		let (slow, ..) = take(0..10, 15);
		let (stopped, ..) = take(4..10, 30);
		assert!(slow > stopped, "{slow:?} over {stopped:?}");
		assert_eq!(stopped, Duration::from_millis(5_966), "from 4.000 to 9.966");

		// A group missing from the middle covers nothing.
		let broadcast = moq_net::broadcast::Info::new().produce();
		let track = broadcast
			.create_track("video", hang::container::track_info(hang::catalog::PRIORITY.video))
			.unwrap();
		write(&track, 0, &second(0));
		write(&track, 2, &second(2));
		let take = Take::new(&target(&broadcast, ts(2_999)), Duration::from_secs(3)).unwrap();
		assert_eq!(take.covered, Duration::from_millis(666 + 666));
	}

	/// A take of a track that wrote nothing in its span, such as a source that
	/// was not encoding, is refused rather than played as nothing.
	#[test]
	fn a_take_of_nothing_is_refused() {
		let (broadcast, _track) = broadcast(0, None);
		let result = Take::new(&target(&broadcast, ts(7_500)), Duration::from_secs(3));
		assert!(matches!(result, Err(Error::Unsupported(_))), "{:?}", result.err());
	}

	#[test]
	fn refuses_a_span_it_cannot_hold() {
		let (broadcast, _track) = broadcast(1, None);
		for length in [Duration::ZERO, MAX_LENGTH + Duration::from_millis(1)] {
			let result = Take::new(&target(&broadcast, ts(500)), length);
			assert!(matches!(result, Err(Error::Invalid(_))), "{length:?}");
		}
	}
}
