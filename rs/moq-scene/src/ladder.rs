//! Simulcast: one picture encoded at several sizes, each its own rendition
//! following its own share of the connection's bandwidth.
//!
//! [`sizes`] lists the rungs: the picture's own size, then smaller ones a
//! viewer on a slower link, or showing it in a corner, picks instead. Each rung
//! is two halves, so a caller can split them across threads:
//!
//! - a [`Gate`] decides, frame by frame, whether the rung encodes and at what
//!   bitrate: its reservation on the connection's
//!   [`Allocator`](moq_net::bandwidth::Allocator), followed through
//!   [`moq_mux::rate`], and whether anyone watches its track;
//! - a [`Rendition`] does what the gate decided: it encodes, retunes, drains
//!   its encoder, and flips the catalog's `enabled` for the rung.
//!
//! A rung above another disables when its grant cannot fund it, so the one
//! below keeps its share, and resumes once the grant recovers. Only a gate
//! marked `always` encodes while nobody watches.

use moq_mux::rate;
use moq_net::bandwidth::{self, Rate};
use moq_net::track::Demand;

use crate::Error;

/// Each rung's size as a fraction of the picture's, largest first.
const SCALES: [(u32, u32); 3] = [(1, 1), (2, 3), (1, 3)];

/// The shortest side a smaller rung may have: below it a picture is too small
/// to be worth a rendition of its own.
const MIN_SIDE: u32 = 180;

/// Bits per pixel per second each rung reserves at most, the default moq's
/// encoders and the JS publisher use: about 4.4 Mb/s for 1080p30.
const BITS_PER_PIXEL: f64 = 0.07;

/// How many rungs a ladder has at most, so a caller can rank its tracks below
/// every rung of another ladder.
pub const RUNGS: usize = SCALES.len();

/// The size an encoder takes for a picture of `size`: even, since the encoder
/// subsamples chroma. An odd picture is scaled down to it.
pub fn encodable(size: moq_video::Size) -> moq_video::Size {
	moq_video::Size::new((size.width & !1).max(2), (size.height & !1).max(2))
}

/// The rungs a picture of `size` is encoded at, largest first: its own
/// [`encodable`] size, then each smaller one whose shorter side is at least
/// [`MIN_SIDE`]. Each keeps the picture's shape.
pub fn sizes(size: moq_video::Size) -> Vec<moq_video::Size> {
	let short = size.width.min(size.height);
	SCALES
		.iter()
		.enumerate()
		.filter(|(index, (num, den))| *index == 0 || short * num / den >= MIN_SIDE)
		.map(|(_, scale)| scaled(size, *scale))
		.collect()
}

/// The `index`th rung of a picture of `size`, as [`sizes`] sizes it, however
/// many rungs the ladder was opened with.
fn rung(size: moq_video::Size, index: usize) -> moq_video::Size {
	scaled(size, SCALES[index])
}

fn scaled(size: moq_video::Size, (num, den): (u32, u32)) -> moq_video::Size {
	let scale = |side: u32| (u64::from(side) * u64::from(num) / u64::from(den)) as u32;
	encodable(moq_video::Size::new(scale(size.width), scale(size.height)))
}

/// The most a rung of `size` at `framerate` sends, which it reserves and
/// encodes at while its grant allows.
pub fn ceiling(size: moq_video::Size, framerate: moq_video::FrameRate) -> Rate {
	Rate::from_bps((size.pixels() as f64 * framerate.as_f64() * BITS_PER_PIXEL) as u64)
}

/// The track priority of rung `index` of `count`, the lowest rung at `base`:
/// each rung above it ranks one lower, so the allocator funds the smaller
/// renditions first and a viewer on a slow link always has one.
pub fn priority(base: i8, index: usize, count: usize) -> i8 {
	base.saturating_sub((count - 1 - index) as i8)
}

/// Publish `rendition` on a new track of `broadcast` at `priority`,
/// registered under `reserved`.
pub fn producer<E: moq_mux::catalog::hang::CatalogExt>(
	broadcast: &moq_net::broadcast::Producer,
	reserved: moq_mux::catalog::Reserved<E>,
	rendition: hang::catalog::VideoConfig,
	priority: i8,
) -> Result<moq_video::encode::Producer, Error> {
	// The suffix `moq_video::encode::Producer::new` derives, which takes no priority.
	let suffix = match &rendition.codec {
		hang::catalog::VideoCodec::H264(_) => ".avc3",
		hang::catalog::VideoCodec::H265(_) => ".hev1",
		other => {
			return Err(Error::Unsupported(format!(
				"{other} is not a codec the studio publishes"
			)));
		}
	};
	let track = broadcast.unique_track(suffix, reserved.track_info(priority))?;
	Ok(moq_video::encode::Producer::with_track(track, reserved, rendition)?)
}

/// What a rung does with the next frame, as its [`Gate`] decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
	/// Encode it at this bitrate.
	Encode(Rate),
	/// Skip it: nobody watches. The rendition stays advertised.
	Idle,
	/// Skip it: the grant cannot fund this rung, so it is advertised disabled
	/// and viewers pick a smaller one.
	Disabled,
}

/// Decides whether one rung encodes, and how fast, from its share of the
/// connection's bandwidth and whether anyone watches it.
///
/// Reads only cheap, synchronous state, so it can run every frame on the
/// thread that produces the frames, even while the rung encodes nothing.
pub struct Gate {
	allocator: bandwidth::Allocator,
	demand: Demand,
	/// Held for as long as the rung publishes: dropping it hands the room back.
	_reservation: bandwidth::Reservation,
	grant: bandwidth::Consumer,
	control: rate::Control,
	always: bool,
}

impl Gate {
	/// Gate the rung publishing to `demand`'s track, reserving `ceiling` on
	/// `allocator`. `lower` is the next rung down's ceiling, below which this
	/// one disables; `None` for the lowest rung, which never does. An `always`
	/// rung encodes while nobody watches.
	pub fn new(
		allocator: &bandwidth::Allocator,
		demand: Demand,
		ceiling: Rate,
		lower: Option<Rate>,
		always: bool,
	) -> Self {
		let reservation = allocator.reserve(&demand, ceiling);
		Self {
			allocator: allocator.clone(),
			grant: reservation.consume(),
			_reservation: reservation,
			control: rate::Control::new(policy(ceiling, lower)),
			demand,
			always,
		}
	}

	/// What the rung does with the next frame.
	pub fn step(&mut self) -> Step {
		// A pure policy fed the current grant on every frame, rather than a
		// follower woken by changes: the frames already tick this often, and a
		// rung that encodes nothing still steps, so it resumes.
		self.control.set_probed(self.grant.probed());
		self.control.update(self.grant.peek(), kio::clock::now());
		match self.control.target() {
			rate::Target::Disabled => Step::Disabled,
			rate::Target::Enabled(rate) if self.always || self.watched() => Step::Encode(rate),
			rate::Target::Enabled(_) => Step::Idle,
		}
	}

	/// Whether anyone subscribes to the rung's track.
	pub fn watched(&self) -> bool {
		self.demand.wanted().is_some()
	}

	/// The rung's share of the connection, as the allocator grants it: `None`
	/// while the connection has no estimate.
	pub fn grant(&self) -> bandwidth::Consumer {
		self.grant.clone()
	}

	/// Follow a rendition that now publishes to `demand`'s track at `ceiling`,
	/// above a rung at `lower`: its picture changed size.
	pub(crate) fn follow(&mut self, demand: Demand, ceiling: Rate, lower: Option<Rate>) {
		let reservation = self.allocator.reserve(&demand, ceiling);
		self.grant = reservation.consume();
		self._reservation = reservation;
		self.control = rate::Control::new(policy(ceiling, lower));
		self.demand = demand;
	}

	/// The track this gate follows.
	pub(crate) fn track(&self) -> &str {
		self.demand.name()
	}
}

fn policy(ceiling: Rate, lower: Option<Rate>) -> rate::Policy {
	let mut policy = rate::Policy::new(ceiling);
	policy.lower = lower;
	policy
}

/// What a [`Rendition`] last did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
	Encoding,
	Idle,
	Disabled,
}

/// One rung's track and encoder, doing what its [`Gate`] decides.
///
/// The encoder opens on the first frame to encode and, unless held, closes
/// while the rung skips frames, so an unwatched rung holds no encoder session.
/// Ends its track cleanly when dropped, with what the encoder still held.
pub struct Rendition {
	/// Taken only to abort it.
	producer: Option<moq_video::encode::Producer>,
	config: moq_video::encode::Config,
	encoder: Option<moq_video::encode::Encoder>,
	/// Keep the encoder, and its session, while skipping frames.
	hold: bool,
	/// A held encoder skipped frames, so its next frame has to be a keyframe.
	resumed: bool,
	state: State,
	/// The bitrate the gate last asked for.
	target: Rate,
	/// The encoder cannot retune, so it stays at its ceiling.
	fixed: bool,
}

impl Rendition {
	/// Encode `config`'s pictures onto `producer`'s track. `config.bitrate` is
	/// the ceiling. A `hold` rendition keeps its encoder while it skips
	/// frames, and resumes with a keyframe.
	pub fn new(producer: moq_video::encode::Producer, config: moq_video::encode::Config, hold: bool) -> Self {
		let target = config
			.bitrate
			.unwrap_or_else(|| ceiling(config.size(), config.framerate));
		Self {
			producer: Some(producer),
			config,
			encoder: None,
			hold,
			resumed: false,
			state: State::Idle,
			target,
			fixed: false,
		}
	}

	/// Open the encoder now rather than on the first frame, so it holds its
	/// session from the start, returning the backend's name.
	pub fn open(&mut self) -> Result<&str, Error> {
		Ok(self.encoder()?.name())
	}

	/// The track's subscriber demand, which moves to a new track when the
	/// picture changes size.
	pub fn demand(&self) -> Demand {
		self.producer().demand()
	}

	/// The size this rendition encodes.
	pub fn size(&self) -> moq_video::Size {
		self.config.size()
	}

	/// Do what the gate decided for the next frame.
	pub fn apply(&mut self, step: Step) -> Result<(), Error> {
		let state = match step {
			Step::Encode(_) => State::Encoding,
			Step::Idle => State::Idle,
			Step::Disabled => State::Disabled,
		};
		if self.state == State::Encoding && state != State::Encoding {
			self.pause()?;
		}
		if (self.state == State::Disabled) != (state == State::Disabled) {
			let enabled = state != State::Disabled;
			self.producer
				.as_mut()
				.expect("open until it ends")
				.set_enabled(enabled)?;
		}
		if state == State::Encoding
			&& std::mem::take(&mut self.resumed)
			&& let Some(encoder) = &mut self.encoder
		{
			// A held encoder resumes where a viewer can start decoding.
			encoder.cut()?;
		}
		self.state = state;
		if let Step::Encode(target) = step {
			self.target = target;
			self.retune()?;
		}
		Ok(())
	}

	/// Encode and publish `frame`, which is [`size`](Self::size). Only after
	/// an [`apply`](Self::apply) of [`Step::Encode`].
	pub fn encode(&mut self, frame: &moq_video::Frame) -> Result<(), Error> {
		debug_assert_eq!(self.state, State::Encoding, "encoding a skipped frame");
		let encoded = self.encoder()?.encode(frame)?;
		self.producer.as_mut().expect("open until it ends").publish(&encoded)?;
		Ok(())
	}

	/// Encode pictures of `size` and `color` from the next frame on, at
	/// `ceiling`. Publishes what the encoder still holds first; the next
	/// encoder moves the rendition to a new track.
	pub(crate) fn resize(
		&mut self,
		size: moq_video::Size,
		color: Option<moq_video::Color>,
		ceiling: Rate,
	) -> Result<(), Error> {
		self.drain()?;
		self.config.width = size.width;
		self.config.height = size.height;
		self.config.color = color;
		self.config.bitrate = Some(ceiling);
		self.target = ceiling;
		self.fixed = false;
		Ok(())
	}

	/// End the track, failed with `err` so viewers see why.
	pub fn abort(mut self, err: &dyn std::fmt::Display) {
		if let Some(producer) = self.producer.take() {
			producer.abort(moq_net::Error::Transport(err.to_string()));
		}
	}

	fn producer(&self) -> &moq_video::encode::Producer {
		self.producer.as_ref().expect("open until it ends")
	}

	/// The open encoder, opening it at the ceiling and retuning it to the
	/// target if it is not.
	fn encoder(&mut self) -> Result<&mut moq_video::encode::Encoder, Error> {
		if self.encoder.is_none() {
			self.encoder = Some(moq_video::encode::Encoder::new(&self.config)?);
			self.fixed = false;
			self.retune()?;
		}
		Ok(self.encoder.as_mut().expect("just opened"))
	}

	/// Move the open encoder to the target bitrate, unless it cannot retune.
	fn retune(&mut self) -> Result<(), Error> {
		let Some(encoder) = self.encoder.as_mut().filter(|_| !self.fixed) else {
			return Ok(());
		};
		match encoder.set_bitrate(self.target) {
			Ok(()) => Ok(()),
			// Not fatal: it keeps encoding at its ceiling, and only stops
			// following the grant.
			Err(moq_video::Error::BitrateUnsupported(name)) => {
				tracing::warn!(encoder = name, "the encoder cannot follow the bandwidth grant");
				self.fixed = true;
				Ok(())
			}
			Err(err) => Err(err.into()),
		}
	}

	/// Stop encoding until the gate asks again: publish what the encoder still
	/// holds, close it unless held, and mark the gap, so a viewer arriving
	/// during it is not served the last group as if it were live.
	fn pause(&mut self) -> Result<(), Error> {
		match self.hold {
			true => {
				if let (Some(encoder), Some(producer)) = (&mut self.encoder, &mut self.producer) {
					producer.publish(&encoder.flush()?)?;
					self.resumed = true;
				}
			}
			false => self.drain()?,
		}
		self.producer.as_mut().expect("open until it ends").discontinuity()?;
		Ok(())
	}

	/// Publish what the encoder still holds and close it, so a pipelined
	/// backend's last pictures are not lost when the encoder closes.
	fn drain(&mut self) -> Result<(), Error> {
		if let (Some(encoder), Some(producer)) = (self.encoder.take(), &mut self.producer) {
			producer.publish(&encoder.finish()?)?;
		}
		Ok(())
	}
}

impl Drop for Rendition {
	fn drop(&mut self) {
		// A source removed or edited, or a studio closing, ends its tracks
		// cleanly rather than leaving subscribers with a dropped one.
		let drained = self.drain();
		match (self.producer.take(), drained) {
			(Some(mut producer), Ok(())) => {
				let _ = producer.finish();
			}
			(Some(producer), Err(err)) => producer.abort(moq_net::Error::Transport(err.to_string())),
			(None, _) => {}
		}
	}
}

/// A picture's rungs, each a [`Gate`] and a [`Rendition`] on one thread.
pub(crate) struct Ladder {
	rungs: Vec<(Gate, Rendition)>,
	framerate: moq_video::FrameRate,
}

impl Ladder {
	/// Rungs `rungs`, largest first, as [`sizes`] lists them.
	pub(crate) fn new(rungs: Vec<(Gate, Rendition)>, framerate: moq_video::FrameRate) -> Self {
		Self { rungs, framerate }
	}

	/// The size of the largest rung, which the others are fractions of.
	pub(crate) fn size(&self) -> moq_video::Size {
		self.rungs[0].1.size()
	}

	/// Encode pictures of `size` and `color` from the next frame on, keeping
	/// the number of rungs.
	pub(crate) fn resize(&mut self, size: moq_video::Size, color: Option<moq_video::Color>) -> Result<(), Error> {
		for (index, (_, rendition)) in self.rungs.iter_mut().enumerate() {
			let size = rung(size, index);
			rendition.resize(size, color, ceiling(size, self.framerate))?;
		}
		Ok(())
	}

	/// Encode `frame` onto every rung its gate lets encode, scaling it to each.
	pub(crate) fn encode(&mut self, frame: &moq_video::Frame) -> Result<(), Error> {
		let ceilings: Vec<Rate> = self
			.rungs
			.iter()
			.map(|(_, rendition)| ceiling(rendition.size(), self.framerate))
			.collect();
		for (index, (gate, rendition)) in self.rungs.iter_mut().enumerate() {
			let step = gate.step();
			rendition.apply(step)?;
			if !matches!(step, Step::Encode(_)) {
				continue;
			}
			let size = rendition.size();
			match frame.size() == size {
				true => rendition.encode(frame)?,
				false => rendition.encode(&frame.resize(size, &Default::default())?)?,
			}
			// A new size moves the rendition to a new track, whose demand the
			// reservation has to follow.
			let demand = rendition.demand();
			if demand.name() != gate.track() {
				gate.follow(demand, ceilings[index], ceilings.get(index + 1).copied());
			}
		}
		Ok(())
	}

	/// End every rung's track, failed with `err`.
	pub(crate) fn abort(self, err: &dyn std::fmt::Display) {
		for (_, rendition) in self.rungs {
			rendition.abort(err);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn rungs_keep_the_shape_down_to_a_floor() {
		let size = |width, height| moq_video::Size::new(width, height);
		assert_eq!(
			sizes(size(1920, 1080)),
			[size(1920, 1080), size(1280, 720), size(640, 360)]
		);
		// A portrait picture scales by its shorter side too.
		assert_eq!(
			sizes(size(1080, 1920)),
			[size(1080, 1920), size(720, 1280), size(360, 640)]
		);
		// Rungs under the floor are dropped, and odd sides round down to even.
		assert_eq!(sizes(size(641, 361)), [size(640, 360), size(426, 240)]);
		assert_eq!(sizes(size(200, 150)), [size(200, 150)]);
	}

	#[test]
	fn smaller_rungs_rank_first() {
		let base = hang::catalog::PRIORITY.video;
		let ranks: Vec<i8> = (0..3).map(|index| priority(base, index, 3)).collect();
		assert_eq!(ranks, [base - 2, base - 1, base]);
	}

	/// A gate follows its share of the estimate down at once and back up over
	/// the ramp, and a rung that is not `always` encodes only while watched.
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

	/// A rung its grant cannot fund is advertised disabled and encodes
	/// nothing, then resumes on a keyframe; an idle one stays on offer. Either
	/// gives its encoder session up, unless it holds it.
	#[tokio::test]
	async fn a_disabled_rung_is_advertised_off() {
		for hold in [false, true] {
			let mut broadcast = moq_net::broadcast::Info::new().produce();
			let catalog = moq_mux::catalog::Producer::<()>::new(&mut broadcast, Default::default()).unwrap();
			let framerate = moq_video::FrameRate::new(30, 1).unwrap();
			let size = moq_video::Size::new(64, 48);
			let mut config = moq_video::encode::Config::new(size.width, size.height, framerate);
			// The one encoder every host has, and one that holds no frames back.
			config.kind = moq_video::encode::Kind::Named("openh264".into());
			config.bitrate = Some(ceiling(size, framerate));
			let rendition = config.probe().await.unwrap();
			let producer = producer(&broadcast, catalog.reserve(), rendition, hang::catalog::PRIORITY.video).unwrap();
			let mut rendition = Rendition::new(producer, config, hold);
			let name = rendition.demand().name().to_string();
			let enabled = || catalog.snapshot().video.renditions[&name].enabled;
			let sent = || catalog.stats()[&name].clone();
			let gray = |millis: u64| {
				let i420 = moq_video::I420::new(size, vec![0x80; moq_video::I420::len(size).unwrap()]).unwrap();
				let timestamp = moq_net::Timestamp::from_micros(millis * 1000).unwrap();
				moq_video::Frame::new(moq_video::Surface::I420(i420), timestamp)
			};
			let encode = Step::Encode(ceiling(size, framerate));

			rendition.apply(encode).unwrap();
			for millis in [0, 33, 66] {
				rendition.encode(&gray(millis)).unwrap();
			}
			assert!(enabled());
			assert_eq!((sent().frames, sent().keyframes), (3, 1));

			rendition.apply(Step::Disabled).unwrap();
			assert!(!enabled(), "viewers pick another rung");
			assert_eq!(rendition.encoder.is_some(), hold, "only a held rung keeps its session");

			rendition.apply(encode).unwrap();
			assert!(enabled());
			for millis in [200, 233] {
				rendition.encode(&gray(millis)).unwrap();
			}
			assert_eq!((sent().frames, sent().keyframes), (5, 2), "it resumes on a keyframe");

			rendition.apply(Step::Idle).unwrap();
			assert!(enabled(), "an unwatched rung stays on offer");
			assert_eq!(rendition.encoder.is_some(), hold, "only a held rung keeps its session");
			rendition.apply(encode).unwrap();
			rendition.encode(&gray(400)).unwrap();
			assert_eq!((sent().frames, sent().keyframes), (6, 3), "it resumes on a keyframe");
		}
	}
}
