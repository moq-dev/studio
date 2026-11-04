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
	_tracks: Vec<moq_net::track::Producer>,
	_listing: moq_mux::catalog::Producer,
	/// The video rendition, under its catalog name.
	pub video: Option<(String, VideoConfig)>,
	/// The audio rendition, under its catalog name.
	pub audio: Option<(String, AudioConfig)>,
	/// How many pictures of the span the video holds: fewer where the
	/// rendition was not encoding, gaps inside the span included.
	pub pictures: usize,
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
		let start = Duration::from(end).saturating_sub(length);
		let start = moq_net::Timestamp::try_from(start)
			.and_then(|start| start.convert(end.scale()))
			.map_err(|err| Error::Invalid(format!("replay start: {err}")))?;

		let mut broadcast = moq_net::broadcast::Info::new().produce();
		let mut tracks = Vec::new();
		let mut video = None;
		let mut pictures = 0;
		if let Some((name, config)) = &target.video {
			reference(config)?;
			let container = moq_mux::catalog::hang::Container::try_from(config)?;
			let track = Reference::track(config, name);
			if let Some((track, held)) = copy(target, &broadcast, track, start, cut, &container)? {
				tracks.push(track);
				pictures = held;
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
			pictures,
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
/// track of `broadcast`, and end that track, with how many pictures of the
/// span it holds. `None` when it holds no media of the span, only the markers
/// of a publisher's gaps if anything, or while its publisher has not
/// described the track. `container` reads its frames.
fn copy(
	target: &Target,
	broadcast: &moq_net::broadcast::Producer,
	name: &str,
	start: moq_net::Timestamp,
	cut: Cut,
	container: &impl moq_mux::container::Container,
) -> Result<Option<(moq_net::track::Producer, usize)>, Error> {
	let range = moq_net::track::Range {
		start: moq_net::track::Anchor::Time(start),
		end: Some(moq_net::track::Anchor::Time(target.end)),
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
	// Never stale: the take keeps its groups for as long as it lives.
	let info = source.info().clone().with_max_age(None);
	let mut track = broadcast.create_track(name, info)?;
	let pictures = match copy_held(&mut source, &mut track, start..=target.end, cut, container) {
		Ok(Some(pictures)) => pictures,
		Ok(None) => {
			let _ = track.abort(moq_net::Error::Cancel);
			return Ok(None);
		}
		Err(err) => {
			let _ = track.clone().abort(moq_net::Error::Cancel);
			return Err(err);
		}
	};
	track.finish()?;
	Ok(Some((track, pictures)))
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
}

/// Copy every group and frame `source` has ready, up to `end` as `cut` says,
/// into `track`, returning how many media frames inside `span` it held, or
/// `None` without media anywhere.
fn copy_held(
	source: &mut moq_net::track::Subscriber,
	track: &mut moq_net::track::Producer,
	span: std::ops::RangeInclusive<moq_net::Timestamp>,
	cut: Cut,
	container: &impl moq_mux::container::Container,
) -> Result<Option<usize>, Error> {
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
		let inside = |frame: &moq_net::frame::Frame| frame.timestamp.is_none_or(|timestamp| timestamp <= end);
		let kept = match cut {
			Cut::Decodable => frames.iter().rposition(inside).map_or(0, |last| last + 1),
			Cut::Shown => frames.iter().position(|frame| !inside(frame)).unwrap_or(frames.len()),
		};
		// None kept is a group wholly past the span, or one the publisher gave
		// up or that aged out before any of it was read, which replays as the
		// gap it is.
		if kept > 0 {
			frames.truncate(kept);
			held.insert(group.sequence, Held { frames, read });
		}
	}

	if let Some(&first) = held.keys().next() {
		track.start_at(first)?;
	}
	for (&sequence, held) in &held {
		let mut into = track.create_group(moq_net::group::Info { sequence })?;
		for frame in &held.frames {
			into.write_frame(frame.timestamp, frame.payload.clone())?;
		}
		match &held.read {
			Ok(()) => into.finish()?,
			Err(err) => into.abort(err.clone())?,
		}
	}

	// The media past the markers of gaps, which may all be from before the
	// span, such as a clip cut back to its keyframe, and how much is inside.
	let media: Vec<_> = held
		.values()
		.flat_map(|held| &held.frames)
		.filter(|frame| !marker(frame, container))
		.collect();
	let pictures = media
		.iter()
		.filter(|frame| frame.timestamp.is_none_or(|at| span.contains(&at)))
		.count();
	let media = !media.is_empty();

	// Groups the span covers that are not here are gaps, not groups to come.
	let copied: Vec<u64> = held.into_keys().collect();
	for pair in copied.windows(2) {
		if pair[1] > pair[0] + 1 {
			track.drop_groups(pair[0] + 1..pair[1], moq_net::StreamError::NotFound);
		}
	}
	Ok(media.then_some(pictures))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn ts(millis: u64) -> moq_net::Timestamp {
		moq_net::Timestamp::from_micros(millis * 1000).unwrap()
	}

	/// Write group `sequence` of `track` with a frame at each of `at`, in
	/// milliseconds and decode order, the frame at `at[i]` named
	/// `{sequence}.{i}`.
	fn write(track: &moq_net::track::Producer, sequence: u64, at: &[u64]) {
		let mut group = track.create_group(moq_net::group::Info { sequence }).unwrap();
		for (index, at) in at.iter().enumerate() {
			group
				.write_frame(ts(*at), format!("{sequence}.{index}").into_bytes())
				.unwrap();
		}
		group.finish().unwrap();
	}

	/// The frames of group `sequence` of [`broadcast`], three a third of a
	/// second apart from its second.
	fn second(sequence: u64) -> [u64; 3] {
		[0, 333, 666].map(|at| sequence * 1000 + at)
	}

	/// A video track with a group a second, from `0` up to but excluding
	/// `groups` seconds, but for the group the publisher gave up at `lost`.
	fn broadcast(groups: u64, lost: Option<u64>) -> (moq_net::broadcast::Producer, moq_net::track::Producer) {
		let broadcast = moq_net::broadcast::Info::new().produce();
		let track = broadcast
			.create_track("video", hang::container::track_info(hang::catalog::PRIORITY.video))
			.unwrap();
		for sequence in 0..groups {
			match lost == Some(sequence) {
				true => track.drop_groups(sequence..sequence + 1, moq_net::StreamError::Cancel),
				false => write(&track, sequence, &second(sequence)),
			}
		}
		(broadcast, track)
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
			groups.push((group.sequence, frames));
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
	/// wait for.
	#[tokio::test(start_paused = true)]
	async fn a_take_keeps_a_gap() {
		let (broadcast, _track) = broadcast(10, Some(5));
		let take = Take::new(&target(&broadcast, ts(7_500)), Duration::from_secs(3)).unwrap();
		assert_eq!(
			read(&take).await,
			[(4, frames(4, 3)), (6, frames(6, 3)), (7, frames(7, 2))]
		);
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
		let mut live = track.create_group(moq_net::group::Info { sequence: 7 }).unwrap();
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
		assert_eq!(take.pictures, 6);

		// The marker a publisher writes as it stops encoding.
		let config = target(&broadcast, ts(0)).video.unwrap().1;
		let container = moq_mux::catalog::hang::Container::try_from(&config).unwrap();
		let mut group = track.create_group(moq_net::group::Info { sequence: 3 }).unwrap();
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
