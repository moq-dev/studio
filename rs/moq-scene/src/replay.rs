//! Instant replay: the recent past of a broadcast, held so it can be played
//! back at any speed or saved.
//!
//! The groups are already on hand. A track keeps its recent groups as far back
//! as its `max_age`, whoever watches, so a [`Take`] copies those covering a span
//! into a broadcast of its own. Nothing ages a take's groups, so a slow-motion
//! replay that ends long after its span left the track's window still has all
//! of it, and its tracks end where the span does, so an export of it finishes.
//! Copying shares each frame's payload rather than duplicating it.

use std::collections::BTreeSet;
use std::future::Future;
use std::time::Duration;

use hang::catalog::{AudioConfig, Reference, VideoConfig};

use crate::Error;

/// The longest span a take holds: as far back as a media track keeps its groups.
///
/// Further back, a track holds nothing to copy, and a relay asked for it would
/// fetch it from an archive, so bounding the span bounds what a take can make
/// the studio hold to that many seconds of the broadcast.
pub const MAX_LENGTH: Duration = hang::container::MAX_AGE;

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
pub struct Take {
	broadcast: moq_net::broadcast::Producer,
	/// Keep the copies and their listing readable once the copy finishes.
	_tracks: Vec<moq_net::track::Producer>,
	_listing: moq_mux::catalog::Producer,
	/// The video rendition, under its catalog name.
	pub video: Option<(String, VideoConfig)>,
	/// The audio rendition, under its catalog name.
	pub audio: Option<(String, AudioConfig)>,
	/// Where the span starts on the copied broadcast's timeline.
	pub start: moq_net::Timestamp,
	/// Where it ends.
	pub end: moq_net::Timestamp,
}

impl Take {
	/// Hold the `length` of `target` that ends at its end, at most [`MAX_LENGTH`],
	/// as far as its tracks hold it now: a frame written after this call is not
	/// copied, so a span ending at the live edge may stop a frame or two short.
	///
	/// Returns the take and the copy that fills it. Run the copy until it
	/// finishes, which needs nothing more to be published, since the take's
	/// tracks end only then. A copy that fails aborts the take's tracks, so
	/// their readers see why.
	///
	/// Refuses a target with neither rendition, one served from another
	/// broadcast, a length of zero or over [`MAX_LENGTH`], and, over a wire that
	/// cannot address a received track by time, a broadcast from a relay.
	pub async fn new(
		target: &Target,
		length: Duration,
	) -> Result<(Self, impl Future<Output = Result<(), Error>> + Send + 'static), Error> {
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
		let mut catalog = hang::Catalog::<()>::default();
		let mut tracks = Vec::new();
		let video = match &target.video {
			Some((name, config)) => {
				reference(config)?;
				catalog.video.renditions.insert(name.clone(), config.clone());
				let (source, track) = open(target, &broadcast, Reference::track(config, name), start).await?;
				tracks.push(track.clone());
				Some(copy(source, track))
			}
			None => None,
		};
		let audio = match &target.audio {
			Some((name, config)) => {
				reference(config)?;
				catalog.audio.renditions.insert(name.clone(), config.clone());
				let (source, track) = open(target, &broadcast, Reference::track(config, name), start).await?;
				tracks.push(track.clone());
				Some(copy(source, track))
			}
			None => None,
		};
		// Listed so an export of the take finds its renditions; HLS is for live
		// viewers, which a take has none of.
		let config = moq_mux::catalog::Config::default().with_hls(None);
		let mut listing = moq_mux::catalog::Producer::new(&mut broadcast, config)?;
		listing.mutate(|listed| {
			listed.video.renditions = catalog.video.renditions;
			listed.audio.renditions = catalog.audio.renditions;
		})?;
		let listed = listing.clone();

		let copy = async move {
			let (video, audio) = tokio::join!(run(video), run(audio));
			let result = video.and(audio);
			match &result {
				Ok(()) => listing.finish()?,
				Err(_) => listing.abort(moq_net::Error::Cancel),
			}
			result
		};
		let take = Self {
			broadcast,
			_tracks: tracks,
			_listing: listed,
			video: target.video.clone(),
			audio: target.audio.clone(),
			start,
			end,
		};
		Ok((take, copy))
	}

	/// The take's broadcast, to decode or export.
	pub fn consume(&self) -> moq_net::broadcast::Consumer {
		self.broadcast.consume()
	}
}

/// Run a copy, if there is one.
async fn run(copy: Option<impl Future<Output = Result<(), Error>>>) -> Result<(), Error> {
	match copy {
		Some(copy) => copy.await,
		None => Ok(()),
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

/// Subscribe to the span of `name` in `target`, and create the track in
/// `broadcast` it is copied to. Also returns the newest frame the track held
/// when asked, which the copy stops at.
async fn open(
	target: &Target,
	broadcast: &moq_net::broadcast::Producer,
	name: &str,
	start: moq_net::Timestamp,
) -> Result<(Copy, moq_net::track::Producer), Error> {
	let range = moq_net::track::Range {
		start: moq_net::track::Anchor::Time(start),
		end: Some(moq_net::track::Anchor::Time(target.end)),
	};
	// Oldest first, so the start of the span is copied ahead of the group still
	// being written at its end, and with the publisher's whole window, so none of
	// it is skipped as stale.
	let subscription = moq_net::track::Subscription::default()
		.with_ranges([range])
		.with_max_delay(None)
		.with_order(moq_net::track::Order::Ascending);
	let mut track = target.broadcast.track(name)?;
	// What is held now, without waiting: a span is the recent past, and a
	// source that has stopped publishing, such as a still screen, may never
	// write past it.
	let held = match track.poll_largest(&kio::Waiter::noop()) {
		std::task::Poll::Ready(held) => held,
		std::task::Poll::Pending => None,
	};
	let source = track.subscribe(subscription).await?;
	// Never stale: the take keeps its groups for as long as it lives.
	let info = source.info().clone().with_max_age(None);
	let track = broadcast.create_track(name, info)?;
	let copy = Copy {
		source,
		held,
		end: target.end,
	};
	Ok((copy, track))
}

/// Where a copy reads from, and how far.
struct Copy {
	source: moq_net::track::Subscriber,
	/// The newest frame held when the take was asked for, or `None` when the
	/// track held nothing.
	held: Option<moq_net::track::Position>,
	end: moq_net::Timestamp,
}

/// Copy each group the span covers into `track`, then end it.
async fn copy(copy: Copy, mut track: moq_net::track::Producer) -> Result<(), Error> {
	let result = copy_groups(copy, &mut track).await;
	match &result {
		Ok(()) => track.finish()?,
		Err(err) => {
			let _ = track.abort(moq_net::Error::Cancel);
			tracing::warn!(%err, "replay copy failed");
		}
	}
	result
}

async fn copy_groups(copy: Copy, track: &mut moq_net::track::Producer) -> Result<(), Error> {
	let Copy { mut source, held, end } = copy;
	let Some(held) = held else {
		return Ok(());
	};
	let mut copied = BTreeSet::new();
	let mut past = false;
	// Oldest first, so once the newest frame held, or one past the end, is
	// copied the span is whole.
	while !past && let Some(mut group) = source.recv_group().await? {
		let sequence = group.sequence;
		if sequence > held.group {
			break;
		}
		let mut into: Option<moq_net::group::Producer> = None;
		let mut index = 0;
		let read = loop {
			if sequence == held.group && index > held.frame {
				past = true;
				break Ok(());
			}
			let frame = match group.read_frame().await {
				Ok(Some(frame)) => frame,
				Ok(None) => break Ok(()),
				Err(err) => break Err(err),
			};
			index += 1;
			if frame.timestamp.is_some_and(|timestamp| timestamp > end) {
				past = true;
				break Ok(());
			}
			let into = match &mut into {
				Some(into) => into,
				None => {
					if copied.is_empty() {
						track.start_at(sequence)?;
					}
					copied.insert(sequence);
					into.insert(track.create_group(moq_net::group::Info { sequence })?)
				}
			};
			into.write_frame(frame.timestamp, frame.payload)?;
		};
		// A group the publisher gave up, or that aged out before it was read,
		// replays as the gap it is.
		match (into, read) {
			(Some(mut into), Ok(())) => into.finish()?,
			(Some(into), Err(err)) => into.abort(err)?,
			(None, _) => {}
		}
	}

	// Groups the span covers that never arrived are gaps, not groups still to come.
	let copied: Vec<u64> = copied.into_iter().collect();
	for pair in copied.windows(2) {
		if pair[1] > pair[0] + 1 {
			track.drop_groups(pair[0] + 1..pair[1], moq_net::StreamError::NotFound);
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn ts(millis: u64) -> moq_net::Timestamp {
		moq_net::Timestamp::from_micros(millis * 1000).unwrap()
	}

	/// A video track with a group a second, each of three frames a third of a
	/// second apart, from `0` up to but excluding `groups` seconds, but for the
	/// group the publisher gave up at `lost`.
	fn broadcast(groups: u64, lost: Option<u64>) -> (moq_net::broadcast::Producer, moq_net::track::Producer) {
		let broadcast = moq_net::broadcast::Info::new().produce();
		let track = broadcast
			.create_track("video", hang::container::track_info(hang::catalog::PRIORITY.video))
			.unwrap();
		for sequence in 0..groups {
			if lost == Some(sequence) {
				track.drop_groups(sequence..sequence + 1, moq_net::StreamError::Cancel);
				continue;
			}
			let mut group = track.create_group(moq_net::group::Info { sequence }).unwrap();
			for frame in 0..3 {
				let at = ts(sequence * 1000 + frame * 333);
				group
					.write_frame(at, format!("{sequence}.{frame}").into_bytes())
					.unwrap();
			}
			group.finish().unwrap();
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
			.subscribe(moq_net::track::Subscription::default().with_max_delay(None))
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
		let (take, copy) = Take::new(&target(&broadcast, ts(7_500)), Duration::from_secs(3))
			.await
			.unwrap();
		assert_eq!(take.start, ts(4_500));
		copy.await.unwrap();
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
		let (take, copy) = Take::new(&target(&broadcast, ts(7_500)), Duration::from_secs(3))
			.await
			.unwrap();
		copy.await.unwrap();
		assert_eq!(
			read(&take).await,
			[(4, frames(4, 3)), (6, frames(6, 3)), (7, frames(7, 2))]
		);
	}

	/// A take at the live edge holds what was written when it was taken, and
	/// ends without waiting for more: a source may never write again, such as a
	/// screen that stopped changing.
	#[tokio::test(start_paused = true)]
	async fn a_take_at_the_live_edge_holds_what_was_written() {
		let (broadcast, track) = broadcast(7, None);
		let mut live = track.create_group(moq_net::group::Info { sequence: 7 }).unwrap();
		live.write_frame(ts(7_000), b"7.0".to_vec()).unwrap();
		let (take, copy) = Take::new(&target(&broadcast, ts(7_500)), Duration::from_secs(3))
			.await
			.unwrap();
		live.write_frame(ts(7_333), b"7.1".to_vec()).unwrap();
		copy.await.unwrap();
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

	#[tokio::test]
	async fn refuses_a_span_it_cannot_hold() {
		let (broadcast, _track) = broadcast(1, None);
		for length in [Duration::ZERO, MAX_LENGTH + Duration::from_millis(1)] {
			let result = Take::new(&target(&broadcast, ts(500)), length).await;
			assert!(matches!(result, Err(Error::Invalid(_))), "{length:?}");
		}
	}
}
