//! The program's simulcast ladder: the canvas, and half of it for a viewer on
//! a slow link or showing the program small.
//!
//! Two sizes, not more, since every rendition that encodes holds an encode
//! session, which consumer GPUs cap. The half size encodes only while someone
//! watches it.
//!
//! Each rendition follows its own share of the connection's bandwidth: a
//! [`Gate`] reserves its ceiling on the connection's
//! [`Allocator`](moq_net::bandwidth::Allocator), follows the grant through
//! [`moq_mux::rate`], and decides frame by frame whether it encodes and how
//! fast. The canvas size disables when its grant cannot fund it over the half
//! size, so viewers move down to the half size, and resumes once it can.

use moq_mux::rate;
use moq_net::bandwidth::{self, Rate};
use moq_net::track::Demand;

/// The shortest side the half size may have: below it a picture is too small
/// to be worth a rendition, or for every hardware encoder to take.
const MIN_SIDE: u32 = 180;

/// Bits per pixel per second each rendition reserves at most, the default
/// moq's encoders and the JS publisher use: about 4.4 Mb/s for 1080p30.
const BITS_PER_PIXEL: f64 = 0.07;

/// The [`track::Info::refresh`](moq_net::track::Info::refresh) ratio each
/// rendition advertises, as moq's own capture does: a viewer joining once a
/// group's pictures outweigh twice its keyframe asks for a keyframe, so later
/// viewers start near one.
pub const REFRESH: u32 = 2;

/// The size an encoder takes for a picture of `size`: even, since the encoder
/// subsamples chroma.
fn encodable(size: moq_video::Size) -> moq_video::Size {
	moq_video::Size::new((size.width & !1).max(2), (size.height & !1).max(2))
}

/// The sizes a canvas of `size` is encoded at, largest first: its own, and
/// half of it unless that is shorter than [`MIN_SIDE`].
pub fn sizes(size: moq_video::Size) -> Vec<moq_video::Size> {
	let half = encodable(moq_video::Size::new(size.width / 2, size.height / 2));
	let mut sizes = vec![encodable(size)];
	if half.width.min(half.height) >= MIN_SIDE {
		sizes.push(half);
	}
	sizes
}

/// The most a rendition of `size` at `framerate` sends, which it reserves and
/// encodes at while its grant allows.
pub fn ceiling(size: moq_video::Size, framerate: moq_video::FrameRate) -> Rate {
	Rate::from_bps((size.pixels() as f64 * framerate.as_f64() * BITS_PER_PIXEL) as u64)
}

/// The track priority of rendition `index` of `count`, largest first: the
/// smallest ranks at the video's, and the canvas size one below it, so the
/// allocator funds the half size first and a viewer on a slow link always has
/// a rendition.
pub fn priority(index: usize, count: usize) -> i8 {
	hang::catalog::PRIORITY.video.saturating_sub((count - 1 - index) as i8)
}

/// What a rendition does with the next frame, as its [`Gate`] decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
	/// Encode it at this bitrate.
	Encode(Rate),
	/// Skip it: nobody watches. The rendition stays advertised.
	Idle,
	/// Skip it: the grant cannot fund this rendition, so it is advertised
	/// disabled and viewers pick the smaller one.
	Disabled,
}

/// Decides whether one rendition encodes, and how fast, from its share of the
/// connection's bandwidth and whether anyone watches it.
///
/// Reads only cheap, synchronous state, so it runs every frame on the program
/// clock, even while the rendition encodes nothing.
pub struct Gate {
	demand: Demand,
	/// Held for as long as the rendition publishes: dropping it hands the
	/// room back.
	_reservation: bandwidth::Reservation,
	grant: bandwidth::Consumer,
	control: rate::Control,
	always: bool,
}

impl Gate {
	/// Gate the rendition publishing to `demand`'s track, reserving `ceiling`
	/// on `allocator`. `lower` is the next rendition down's ceiling, below which
	/// this one disables; `None` for the smallest, which never does. An
	/// `always` rendition encodes while nobody watches.
	pub fn new(
		allocator: &bandwidth::Allocator,
		demand: Demand,
		ceiling: Rate,
		lower: Option<Rate>,
		always: bool,
	) -> Self {
		let reservation = allocator.reserve(&demand, ceiling);
		let mut policy = rate::Policy::new(ceiling);
		policy.lower = lower;
		Self {
			grant: reservation.consume(),
			_reservation: reservation,
			control: rate::Control::new(policy),
			demand,
			always,
		}
	}

	/// What the rendition does with the next frame.
	pub fn step(&mut self) -> Step {
		// A pure policy fed the current grant on every frame, rather than a
		// follower woken by changes: the frames already tick this often, and a
		// rendition that encodes nothing still steps, so it resumes.
		self.control.set_probed(self.grant.probed());
		self.control.update(self.grant.peek(), kio::clock::now());
		match self.control.target() {
			rate::Target::Disabled => Step::Disabled,
			rate::Target::Enabled(rate) if self.always || self.watched() => Step::Encode(rate),
			rate::Target::Enabled(_) => Step::Idle,
		}
	}

	/// Whether anyone subscribes to the rendition's track.
	pub fn watched(&self) -> bool {
		self.demand.wanted().is_some()
	}

	/// The rendition's share of the connection, as the allocator grants it.
	pub fn grant(&self) -> bandwidth::Consumer {
		self.grant.clone()
	}

	/// The name of the rendition's track.
	pub fn track(&self) -> &str {
		self.demand.name()
	}
}

/// When a viewer joining a track asks for a keyframe: past the newest group,
/// as the track's [`REFRESH`] ratio decides.
pub struct Joins {
	track: moq_net::track::Producer,
	/// The newest group a cut was made for, which joins still waiting on that
	/// keyframe ask for too.
	cut: Option<u64>,
}

impl Joins {
	/// Follow the joins of `track`.
	pub fn new(track: moq_net::track::Producer) -> Self {
		Self { track, cut: None }
	}

	/// Whether the next picture should open a group, as a keyframe, for a
	/// viewer that joined. True once per asked-for group, however many
	/// pictures the encoder holds before writing it.
	pub fn due(&mut self) -> bool {
		let largest = self.track.largest().map(|largest| largest.group);
		let asked = self
			.track
			.demand()
			.wanted()
			.filter(|wanted| largest.is_none_or(|largest| *wanted > largest));
		let due = asked > self.cut;
		self.cut = self.cut.max(asked);
		due
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_half_size_keeps_the_shape_down_to_a_floor() {
		let size = |width, height| moq_video::Size::new(width, height);
		assert_eq!(sizes(size(1920, 1080)), [size(1920, 1080), size(960, 540)]);
		assert_eq!(sizes(size(1080, 1920)), [size(1080, 1920), size(540, 960)], "portrait");
		assert_eq!(sizes(size(642, 362)), [size(642, 362), size(320, 180)]);
		assert_eq!(sizes(size(641, 359)), [size(640, 358)], "too small to halve");
	}

	#[test]
	fn the_half_size_ranks_first() {
		let base = hang::catalog::PRIORITY.video;
		assert_eq!([priority(0, 2), priority(1, 2)], [base - 1, base]);
		assert_eq!(priority(0, 1), base);
	}

	/// A gate follows its share of the estimate down at once and back up over
	/// the ramp, and one that is not `always` encodes only while watched.
	#[tokio::test(start_paused = true)]
	async fn a_gate_follows_its_grant_and_its_viewers() {
		let broadcast = moq_net::broadcast::Info::new().produce();
		let consumer = broadcast.consume();
		let track = broadcast.create_track("video", None).unwrap();
		let estimate = bandwidth::Producer::new();
		estimate.set(Some(Rate::from_mbps(3))).unwrap();
		let allocator = bandwidth::Allocator::new(estimate.consume());
		let ceiling = Rate::from_mbps(2);

		let mut always = Gate::new(&allocator, track.demand(), ceiling, None, true);
		assert_eq!(always.step(), Step::Encode(ceiling));
		estimate.set(Some(Rate::from_mbps(1))).unwrap();
		assert_eq!(
			always.step(),
			Step::Encode(Rate::from_mbps(1)),
			"a drop applies at once"
		);
		assert_eq!(always.grant().peek(), Some(Rate::from_mbps(1)));
		estimate.set(Some(Rate::from_mbps(3))).unwrap();
		match always.step() {
			Step::Encode(rate) => assert!(rate < ceiling, "a raise climbs: {rate:?}"),
			step => panic!("{step:?}"),
		}
		tokio::time::advance(std::time::Duration::from_secs(5)).await;
		assert_eq!(always.step(), Step::Encode(ceiling), "and reaches the ceiling");
		drop(always);

		let mut watched = Gate::new(&allocator, track.demand(), ceiling, None, false);
		assert_eq!(watched.step(), Step::Idle, "nobody watches");
		let subscription = consumer.track("video").unwrap().subscribe(None).await.unwrap();
		assert_eq!(watched.step(), Step::Encode(ceiling));
		drop(subscription);
		tokio::task::yield_now().await;
		assert_eq!(watched.step(), Step::Idle, "the viewer left");
	}

	/// A join past the newest group asks for one cut, however many pictures
	/// pass before its keyframe lands, and a group that lands answers it.
	#[tokio::test]
	async fn a_join_cuts_once() {
		let broadcast = moq_net::broadcast::Info::new().produce();
		let consumer = broadcast.consume();
		let info = moq_net::track::Info::default().with_refresh(REFRESH);
		let track = broadcast.create_track("video", info).unwrap();
		let mut joins = Joins::new(track.clone());
		let write = |sequence: u64, bytes: usize| {
			let mut group = track.create_group(moq_net::group::Info { sequence }).unwrap();
			group.write_frame(None, vec![0u8; bytes]).unwrap();
			group
		};
		assert!(!joins.due(), "nobody watches");
		let _first = consumer.track("video").unwrap().subscribe(None).await.unwrap();
		// A first viewer of an empty track asks for its first group, which a
		// fresh encoder opens with anyway.
		assert!(joins.due());
		let mut group = write(0, 100);
		assert!(!joins.due(), "the group landed");
		// Deltas outweighing twice the keyframe make the next join ask.
		group.write_frame(None, vec![0u8; 300]).unwrap();
		let _second = consumer.track("video").unwrap().subscribe(None).await.unwrap();
		assert!(joins.due(), "a join past the newest group");
		assert!(!joins.due(), "the keyframe is still in the encoder");
		group.finish().unwrap();
		let _group = write(1, 100);
		assert!(!joins.due(), "group 1 answered it");
	}
}
