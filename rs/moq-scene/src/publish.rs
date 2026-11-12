//! Each local source's own track, kept in a broadcast of the studio's own.
//!
//! A source's track is for the studio's own use, such as replaying the source
//! or recording it on its own, and is never announced to a relay: the program
//! is the only video the studio publishes, which keeps it within the encode
//! sessions a GPU allows. Its renditions are listed in a hang catalog of its
//! own, nested by source name under the local catalog's [`SECTION`] section.
//!
//! A track encodes only while it is used: while it is on program
//! ([`Publish::set_on_program`]), a picture shown in a scene on air or a sound
//! heard in the program mix, so a replay of it has its recent past, or while
//! something in the studio subscribes to it, such as a recording.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::Error;

/// The local catalog's own root section, namespaced as hang asks of
/// application sections.
pub const SECTION: &str = "live.kixel.studio";

/// The application sections of the local catalog.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ext {
	/// The [`SECTION`] section, left off the wire while nothing is listed.
	#[serde(rename = "live.kixel.studio", default, skip_serializing_if = "Section::is_empty")]
	pub studio: Section,
}

impl moq_mux::catalog::hang::CatalogExt for Ext {}

/// What the local catalog lists.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
	/// Each source's catalog, by source name. Its tracks are in the local
	/// broadcast, and its timestamps are on the program's clock.
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub sources: BTreeMap<String, hang::Catalog>,
}

impl Section {
	fn is_empty(&self) -> bool {
		self.sources.is_empty()
	}
}

/// The catalog of the program's broadcast, or of the sources' local one.
pub type Catalog = moq_mux::catalog::Producer<Ext>;

/// How many video encoders are open. Each holds a session on a hardware
/// encoder, which a GPU caps, such as at 8 on a GeForce card.
#[derive(Clone, Debug, Default)]
pub struct Sessions(Arc<AtomicUsize>);

impl Sessions {
	/// How many encoders are open now.
	pub fn open(&self) -> usize {
		self.0.load(Ordering::Relaxed)
	}

	fn hold(&self) -> Session {
		self.0.fetch_add(1, Ordering::Relaxed);
		Session(self.clone())
	}
}

/// One open encoder's place in its [`Sessions`], given back when dropped.
struct Session(Sessions);

impl Drop for Session {
	fn drop(&mut self) {
		(self.0).0.fetch_sub(1, Ordering::Relaxed);
	}
}

/// Where sources publish: a broadcast the studio keeps to itself, and its
/// catalog.
#[derive(Clone)]
pub struct Publish {
	broadcast: moq_net::broadcast::Producer,
	catalog: Catalog,
	kind: moq_video::encode::Kind,
	sessions: Sessions,
	owners: Arc<Mutex<Owners>>,
	/// The sources on program, which each source's track follows.
	on_program: Arc<tokio::sync::watch::Sender<OnProgram>>,
	/// The bandwidth granted each source published independently, by name,
	/// which its video encodes at.
	grants: Grants,
}

/// The bandwidth granted each source published independently, by name, with
/// the forwarded video track it was reserved for.
type Grants = Arc<Mutex<BTreeMap<String, (String, moq_net::bandwidth::Consumer)>>>;

/// The sources on program, by name, apart for pictures and sound: a media
/// file heard in the program mix but shown in no scene on air encodes its
/// sound and not its pictures.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OnProgram {
	/// Those whose picture a scene on air shows.
	pub shown: BTreeSet<String>,
	/// Those whose sound the program mix plays.
	pub heard: BTreeSet<String>,
}

/// Which [`Claim`] owns each source name's entry in the local catalog, and
/// each open [`Listing`]'s catalog by its claim.
#[derive(Default)]
struct Owners {
	next: u64,
	by_name: BTreeMap<String, u64>,
	catalogs: BTreeMap<u64, moq_mux::catalog::Producer>,
}

impl Publish {
	/// Keep sources' tracks in a new local broadcast stamped on `clock`, the
	/// program's, encoding captured video with `kind` and counting its open
	/// encoders in `sessions`.
	///
	/// Pass [`Kind::Hardware`](moq_video::encode::Kind::Hardware) on a machine
	/// with a hardware encoder, so a session it refuses fails that source's track
	/// with the reason rather than quietly loading the CPU with a software one.
	pub fn new(clock: moq_mux::Clock, kind: moq_video::encode::Kind, sessions: Sessions) -> Result<Self, Error> {
		let mut broadcast = moq_net::broadcast::Info::new().produce();
		let config = moq_mux::catalog::Config::default()
			.with_catalog(Default::default())
			.with_clock(clock);
		let catalog = Catalog::new(&mut broadcast, config)?;
		Ok(Self {
			broadcast,
			catalog,
			kind,
			sessions,
			owners: Default::default(),
			on_program: Default::default(),
			grants: Default::default(),
		})
	}

	/// The local broadcast, to read a source's track from.
	pub fn consume(&self) -> moq_net::broadcast::Consumer {
		self.broadcast.consume()
	}

	/// What the local catalog lists now.
	pub fn catalog(&self) -> hang::Catalog<Ext> {
		self.catalog.snapshot()
	}

	/// End the local catalog, once every source has stopped, so whatever reads
	/// it reaches its end rather than a cut.
	pub fn finish(&self) -> Result<(), Error> {
		Ok(self.catalog.clone().finish()?)
	}

	/// Encode the picture of each source `on_program` shows and the sound of
	/// each it hears, whether or not anything reads their tracks; any other
	/// track encodes only while read.
	pub fn set_on_program(&self, on_program: OnProgram) {
		// Only a change wakes the sources, since this runs every frame.
		self.on_program.send_if_modified(|current| {
			let changed = *current != on_program;
			*current = on_program;
			changed
		});
	}

	/// Publish each source `published` names in `broadcast`, the program's,
	/// as long as it stays named, until `published` closes: its tracks are
	/// copied in under [`FORWARDED`] and its catalog listed under `catalog`'s
	/// [`SECTION`] section, renamed to match. Reading its tracks keeps each
	/// source encoding while it is published. Each copied audio or video
	/// track claims its bitrate from `bandwidth` while subscribed, as the
	/// program's tracks do.
	pub async fn forward(
		&self,
		mut published: tokio::sync::watch::Receiver<BTreeSet<String>>,
		broadcast: moq_net::broadcast::Producer,
		mut catalog: Catalog,
		bandwidth: moq_net::bandwidth::Allocator,
	) -> Result<(), Error> {
		let mut updates = self.catalog.consume()?;
		let mut local = self.catalog.snapshot();
		// Each copied track's copy, by its local name, aborted once dropped.
		let mut copies: BTreeMap<String, tokio::task::JoinHandle<()>> = BTreeMap::new();
		loop {
			let listed: BTreeMap<String, hang::Catalog> = {
				let published = published.borrow_and_update();
				let sources = local.ext.studio.sources.iter();
				let sources = sources.filter(|(name, _)| published.contains(*name));
				sources.map(|(name, source)| (name.clone(), source.clone())).collect()
			};
			let tracks: BTreeMap<String, (String, Option<(moq_net::bandwidth::Rate, bool)>)> = listed
				.iter()
				.flat_map(|(source, catalog)| tracks(catalog).map(move |(name, rate)| (name, (source.clone(), rate))))
				.collect();
			copies.retain(|name, copy| {
				let keep = tracks.contains_key(name);
				if !keep {
					copy.abort();
				}
				keep
			});
			for (name, (source, rate)) in tracks {
				if copies.contains_key(&name) {
					continue;
				}
				let bandwidth = rate.map(|(rate, video)| Grant {
					bandwidth: bandwidth.clone(),
					rate,
					video,
					grants: self.grants.clone(),
					source,
				});
				let copy = copy(self.consume(), broadcast.clone(), name.clone(), bandwidth);
				copies.insert(name, tokio::spawn(copy));
			}
			let listed: BTreeMap<String, hang::Catalog> = listed
				.into_iter()
				.map(|(name, source)| (name, renamed(source)))
				.collect();
			catalog.mutate(|root| {
				if root.ext.studio.sources != listed {
					root.ext.studio.sources = listed;
				}
			})?;

			tokio::select! {
				next = updates.next() => match next? {
					Some(next) => local = next,
					None => break,
				},
				changed = published.changed() => if changed.is_err() { break },
			}
		}
		for copy in copies.into_values() {
			copy.abort();
		}
		catalog.mutate(|root| root.ext.studio.sources.clear())?;
		Ok(())
	}

	/// What each source track the local catalog lists has written, keyed by
	/// rendition ID, as [`Catalog::stats`](moq_mux::catalog::Producer::stats)
	/// reports the program's. The broadcast names every track uniquely, so all
	/// sources share one map. A replaced source's tracks drop out with its
	/// catalog entry, even while it is still stopping.
	pub fn stats(&self) -> BTreeMap<String, hang::stats::Rendition> {
		let owners = self.owners.lock().unwrap();
		let listed = owners.by_name.values().filter_map(|id| owners.catalogs.get(id));
		listed.flat_map(|catalog| catalog.stats()).collect()
	}

	/// Claim `name`'s entry in the local catalog for a source that is opening,
	/// taking it from any older source of that name.
	pub(crate) fn claim(&self, name: &str) -> Claim {
		let mut owners = self.owners.lock().unwrap();
		let id = owners.next;
		owners.next += 1;
		owners.by_name.insert(name.to_string(), id);
		Claim {
			name: name.to_string(),
			id,
			owners: self.owners.clone(),
		}
	}
}

/// The prefix of each source track's name in the program's broadcast, ahead
/// of its name in the local one, so it never meets one of the program's own.
pub const FORWARDED: &str = "source/";

/// What a source's video claims of the connection when its catalog names
/// no bitrate.
const VIDEO_BITRATE: moq_net::bandwidth::Rate = moq_net::bandwidth::Rate::from_kbps(6000);

/// What a source's sound claims when its catalog names no bitrate.
const AUDIO_BITRATE: moq_net::bandwidth::Rate = moq_net::bandwidth::Rate::from_kbps(128);

/// Every track `source`'s catalog lists, by its name in the local broadcast,
/// with the bitrate it claims of the connection: none for a still, sent once.
/// Video is marked, since only its grant is followed: Opus at this rate is
/// never what congests the uplink, as the program's audio.
fn tracks(source: &hang::Catalog) -> impl Iterator<Item = (String, Option<(moq_net::bandwidth::Rate, bool)>)> + '_ {
	let rate = |bitrate: Option<u64>, default| bitrate.map_or(default, moq_net::bandwidth::Rate::from_bps);
	let video = source.video.renditions.iter();
	let video = video.map(move |(name, config)| (name.clone(), Some((rate(config.bitrate, VIDEO_BITRATE), true))));
	let audio = source.audio.renditions.iter();
	let audio = audio.map(move |(name, config)| (name.clone(), Some((rate(config.bitrate, AUDIO_BITRATE), false))));
	let binary = source.binary.tracks.keys().map(|name| (name.clone(), None));
	video.chain(audio).chain(binary)
}

/// `source`'s catalog with each track named as the program's broadcast has it.
fn renamed(mut source: hang::Catalog) -> hang::Catalog {
	fn rename<T>(map: &mut BTreeMap<String, T>) {
		*map = std::mem::take(map)
			.into_iter()
			.map(|(name, config)| (format!("{FORWARDED}{name}"), config))
			.collect();
	}
	rename(&mut source.video.renditions);
	rename(&mut source.audio.renditions);
	rename(&mut source.binary.tracks);
	source
}

/// Copy the local track `name` into `broadcast` under [`FORWARDED`], group by
/// group, until it ends. A failure ends the copy, failed, with the reason.
/// How a copied track claims its bitrate of the connection, and where its
/// source's video finds what it was granted.
struct Grant {
	bandwidth: moq_net::bandwidth::Allocator,
	rate: moq_net::bandwidth::Rate,
	/// Whether the track is video, whose source follows the grant.
	video: bool,
	grants: Grants,
	source: String,
}

/// A copy's reservation, and for a video, its listing for the source to
/// follow, taken out once the copy ends unless a newer copy replaced it.
struct Granted {
	_reservation: moq_net::bandwidth::Reservation,
	grants: Grants,
	/// The source and the copied track, for a video.
	listed: Option<(String, String)>,
}

impl Drop for Granted {
	fn drop(&mut self) {
		let Some((source, track)) = &self.listed else { return };
		let mut grants = self.grants.lock().unwrap();
		if grants.get(source).is_some_and(|(listed, _)| listed == track) {
			grants.remove(source);
		}
	}
}

async fn copy(
	local: moq_net::broadcast::Consumer,
	broadcast: moq_net::broadcast::Producer,
	name: String,
	grant: Option<Grant>,
) {
	let copied = async {
		let mut track = local.track(&name)?.subscribe(None).await?;
		let out = broadcast.create_track(format!("{FORWARDED}{name}"), track.info().clone())?;
		// Held for as long as the copy runs.
		// Held for as long as the copy runs. A video's grant is what its source
		// encodes at, so the source follows the connection as the program does.
		let _grant = grant.map(|grant| {
			let reservation = grant.bandwidth.reserve(&out.demand(), grant.rate);
			let listed = grant.video.then(|| {
				let entry = (out.name().to_string(), reservation.consume());
				grant.grants.lock().unwrap().insert(grant.source.clone(), entry);
				(grant.source, out.name().to_string())
			});
			Granted {
				_reservation: reservation,
				grants: grant.grants,
				listed,
			}
		});
		let result: Result<(), moq_net::Error> = async {
			while let Some(mut group) = track.recv_group().await? {
				let mut copy = out.create_group(moq_net::group::Info {
					sequence: group.sequence,
				})?;
				while let Some(frame) = group.read_frame().await? {
					copy.write_frame(frame.timestamp, frame.payload)?;
				}
				copy.finish()?;
			}
			out.finish()
		}
		.await;
		if let Err(err) = &result {
			let _ = out.clone().abort(err.clone());
		}
		result
	};
	if let Err(err) = copied.await {
		tracing::warn!(track = %name, %err, "cannot publish a source's track");
	}
}

/// A source's hold on its name's entry in the local catalog, until a newer
/// source of that name claims it.
///
/// An edited source's replacement starts before the old one stops, and their
/// drivers run wherever their caller puts them, so the claim is taken in the
/// order the sources open rather than the order their drivers run.
pub(crate) struct Claim {
	name: String,
	id: u64,
	owners: Arc<Mutex<Owners>>,
}

impl Claim {
	/// Whether no newer source of this name has opened since.
	fn owned(&self) -> bool {
		self.owners.lock().unwrap().by_name.get(&self.name) == Some(&self.id)
	}
}

impl Drop for Claim {
	fn drop(&mut self) {
		let mut owners = self.owners.lock().unwrap();
		if owners.by_name.get(&self.name) == Some(&self.id) {
			owners.by_name.remove(&self.name);
		}
	}
}

/// One source's catalog, listed in the local one for as long as this lives
/// and its claim holds.
pub(crate) struct Listing<'a> {
	claim: &'a Claim,
	kind: moq_video::encode::Kind,
	sessions: Sessions,
	on_program: tokio::sync::watch::Receiver<OnProgram>,
	grants: Grants,
	broadcast: moq_net::broadcast::Producer,
	studio: Catalog,
	catalog: moq_mux::catalog::Producer,
	/// Where the nested catalog writes its own track, which nobody reads: its
	/// content reaches viewers through the local catalog instead.
	_scratch: moq_net::broadcast::Producer,
}

impl<'a> Listing<'a> {
	pub(crate) fn new(claim: &'a Claim, publish: &Publish) -> Result<Self, Error> {
		let mut scratch = moq_net::broadcast::Info::new().produce();
		// The program's clock, so source and program timestamps share one timeline.
		let config = moq_mux::catalog::Config::default().with_clock(publish.catalog.clock());
		let catalog = moq_mux::catalog::Producer::new(&mut scratch, config)?;
		claim.owners.lock().unwrap().catalogs.insert(claim.id, catalog.clone());
		Ok(Self {
			claim,
			kind: publish.kind.clone(),
			sessions: publish.sessions.clone(),
			on_program: publish.on_program.subscribe(),
			grants: publish.grants.clone(),
			broadcast: publish.broadcast.clone(),
			studio: publish.catalog.clone(),
			catalog,
			_scratch: scratch,
		})
	}

	/// Copy every update of this source's catalog into the local one, until
	/// either catalog closes.
	pub(crate) async fn mirror(&self) -> Result<(), Error> {
		let mut updates = self.catalog.consume()?;
		let mut studio = self.studio.clone();
		while let Some(mut catalog) = updates.next().await? {
			// Media timing is the root's: the one clock, and the join interval an
			// HLS export numbers its segments on.
			catalog.media = Default::default();
			// The claim is checked inside the mutation, so a newer source's
			// writes, made under the same lock, are never overwritten.
			studio.mutate(|root| {
				if !self.claim.owned() {
					return;
				}
				let name = &self.claim.name;
				let sources = &mut root.ext.studio.sources;
				if catalog == hang::Catalog::default() {
					sources.remove(name);
				} else {
					sources.insert(name.clone(), catalog);
				}
			})?;
		}
		Ok(())
	}
}

impl Drop for Listing<'_> {
	fn drop(&mut self) {
		self.claim.owners.lock().unwrap().catalogs.remove(&self.claim.id);
		// Fails only once the local catalog closed, which took the entry with it.
		let _ = self.studio.mutate(|root| {
			if self.claim.owned() {
				root.ext.studio.sources.remove(&self.claim.name);
			}
		});
		// Fails only once the source's catalog failed, which ended its tracks.
		let _ = self.catalog.finish();
	}
}

/// What an [`Encoding`] needs of its encoder: a
/// [`moq_video::encode::Encoder`], or a test's stand-in that holds pictures the
/// way a pipelined hardware backend does.
pub trait Encode {
	/// [`Encoder::encode`](moq_video::encode::Encoder::encode).
	fn encode(&mut self, frame: &moq_video::Frame) -> Result<Vec<moq_video::encode::Encoded>, moq_video::Error>;
	/// [`Encoder::flush`](moq_video::encode::Encoder::flush).
	fn flush(&mut self) -> Result<Vec<moq_video::encode::Encoded>, moq_video::Error>;
	/// [`Encoder::finish`](moq_video::encode::Encoder::finish).
	fn finish(self) -> Result<Vec<moq_video::encode::Encoded>, moq_video::Error>;
	/// [`Encoder::cut`](moq_video::encode::Encoder::cut).
	fn cut(&mut self) -> Result<(), moq_video::Error>;
	/// [`Encoder::set_bitrate`](moq_video::encode::Encoder::set_bitrate).
	fn set_bitrate(&mut self, bitrate: moq_net::bandwidth::Rate) -> Result<(), moq_video::Error>;
}

impl Encode for moq_video::encode::Encoder {
	fn encode(&mut self, frame: &moq_video::Frame) -> Result<Vec<moq_video::encode::Encoded>, moq_video::Error> {
		self.encode(frame)
	}

	fn flush(&mut self) -> Result<Vec<moq_video::encode::Encoded>, moq_video::Error> {
		self.flush()
	}

	fn finish(self) -> Result<Vec<moq_video::encode::Encoded>, moq_video::Error> {
		self.finish()
	}

	fn cut(&mut self) -> Result<(), moq_video::Error> {
		self.cut()
	}

	fn set_bitrate(&mut self, bitrate: moq_net::bandwidth::Rate) -> Result<(), moq_video::Error> {
		self.set_bitrate(bitrate)
	}
}

/// An encoder and the video track it publishes into.
///
/// A pipelined backend still holds its last pictures when its encoder is
/// replaced, the track pauses, or the track ends, so each of those publishes
/// them first. Dropping this ends the track cleanly rather than leaving
/// subscribers with a dropped one, and a drain that fails aborts the track with
/// the reason.
pub struct Encoding<E: Encode = moq_video::encode::Encoder> {
	/// Taken only to abort or finish it, as the track ends.
	producer: Option<moq_video::encode::Producer>,
	/// The open encoder, and its place among the open sessions.
	encoder: Option<(E, Session)>,
	sessions: Sessions,
}

impl<E: Encode> Encoding<E> {
	/// Publish into `producer`, with no encoder open yet, counting each one
	/// opened in `sessions`.
	pub fn new(producer: moq_video::encode::Producer, sessions: Sessions) -> Self {
		Self {
			producer: Some(producer),
			encoder: None,
			sessions,
		}
	}

	/// Whether an encoder is open.
	pub fn is_open(&self) -> bool {
		self.encoder.is_some()
	}

	/// Whether anything reads the track.
	pub fn watched(&self) -> bool {
		self.demand().wanted().is_some()
	}

	/// Who reads the track, which moves with it when a new encoder writes
	/// another kind of picture.
	pub fn demand(&self) -> moq_net::track::Demand {
		self.producer.as_ref().expect("open until it ends").demand()
	}

	/// Encode with `encoder` from here on, after draining the one it replaces.
	pub fn open(&mut self, encoder: E) -> Result<(), Error> {
		self.drain()?;
		self.encoder = Some((encoder, self.sessions.hold()));
		Ok(())
	}

	/// Encode `frame` and publish what the encoder hands back.
	///
	/// # Panics
	///
	/// When no encoder is [open](Self::open).
	pub fn encode(&mut self, frame: &moq_video::Frame) -> Result<(), Error> {
		let (encoder, _) = self.encoder.as_mut().expect("an encoder is open");
		let encoded = encoder.encode(frame)?;
		self.producer.as_mut().expect("open until it ends").publish(&encoded)?;
		Ok(())
	}

	/// Open a group with the next picture, as a keyframe.
	///
	/// # Panics
	///
	/// When no encoder is [open](Self::open).
	pub fn cut(&mut self) -> Result<(), Error> {
		let (encoder, _) = self.encoder.as_mut().expect("an encoder is open");
		Ok(encoder.cut()?)
	}

	/// Encode at `bitrate` from the next picture on.
	///
	/// # Panics
	///
	/// When no encoder is [open](Self::open).
	pub fn set_bitrate(&mut self, bitrate: moq_net::bandwidth::Rate) -> Result<(), Error> {
		let (encoder, _) = self.encoder.as_mut().expect("an encoder is open");
		Ok(encoder.set_bitrate(bitrate)?)
	}

	/// Advertise whether the track is encoding, as the catalog's `enabled`,
	/// so viewers pick another rendition while it is not.
	pub fn set_enabled(&mut self, enabled: bool) -> Result<(), Error> {
		let producer = self.producer.as_mut().expect("open until it ends");
		producer.set_enabled(enabled)?;
		Ok(())
	}

	/// Close the encoder, publishing what it still holds.
	pub fn drain(&mut self) -> Result<(), Error> {
		if let (Some((encoder, _session)), Some(producer)) = (self.encoder.take(), &mut self.producer) {
			producer.publish(&encoder.finish()?)?;
		}
		Ok(())
	}

	/// Mark a gap in the track after [draining](Self::drain) the encoder, which
	/// gives its session back, so the next one [opened](Self::open) starts with
	/// a keyframe, even on a backend that cannot [cut](Encode::cut).
	pub fn discontinuity(&mut self) -> Result<(), Error> {
		self.drain()?;
		self.producer.as_mut().expect("open until it ends").discontinuity()?;
		Ok(())
	}

	/// Mark a gap in the track, such as the last viewer leaving an on-demand
	/// one, keeping the encoder and its session open. What the encoder still
	/// holds lands before the gap rather than after it, and the next picture
	/// is a keyframe, so a viewer arriving after the gap starts there.
	pub fn pause(&mut self) -> Result<(), Error> {
		let producer = self.producer.as_mut().expect("open until it ends");
		if let Some((encoder, _)) = &mut self.encoder {
			producer.publish(&encoder.flush()?)?;
			encoder.cut()?;
		}
		producer.discontinuity()?;
		Ok(())
	}

	/// End the track cleanly, after publishing what the encoder still holds.
	/// A failure aborts the track with the reason.
	pub fn finish(mut self) -> Result<(), Error> {
		self.end()
	}

	/// End the track, failed with `err` so viewers see why, dropping what the
	/// encoder still holds.
	pub fn abort(mut self, err: moq_net::Error) {
		self.encoder = None;
		if let Some(producer) = self.producer.take() {
			producer.abort(err);
		}
	}

	fn end(&mut self) -> Result<(), Error> {
		let drained = self.drain();
		let Some(mut producer) = self.producer.take() else {
			return drained;
		};
		let result = drained.and_then(|()| Ok(producer.finish()?));
		if let Err(err) = &result {
			producer.abort(moq_net::Error::Transport(err.to_string()));
		}
		result
	}
}

impl<E: Encode> Drop for Encoding<E> {
	fn drop(&mut self) {
		let _ = self.end();
	}
}

/// A captured or replayed source's video, encoded frame by frame as it is
/// shown, while it is used.
pub(crate) struct Video<'a> {
	track: Encoding,
	claim: &'a Claim,
	/// Where a grant is listed while the source is published independently.
	grants: Grants,
	/// The grant the open encoder encodes at, if any.
	granted: Option<moq_net::bandwidth::Rate>,
	/// The sources on program.
	on_program: tokio::sync::watch::Receiver<OnProgram>,
	config: moq_video::encode::Config,
	clock: moq_mux::Clock,
	/// Where the capture's private timeline starts on the studio clock, in
	/// microseconds, taken at the first frame unless [anchored](Self::anchor).
	epoch: Option<u128>,
}

impl<'a> Video<'a> {
	/// Advertise a rendition for pictures of `size`. Opens a throwaway encoder
	/// to learn exactly what the real one will write, so a machine that cannot
	/// encode the picture at all fails here with the reason.
	pub(crate) async fn new(
		listing: &Listing<'a>,
		size: moq_video::Size,
		framerate: moq_video::FrameRate,
		color: Option<moq_video::Color>,
	) -> Result<Self, Error> {
		let size = encodable(size);
		let mut config = moq_video::encode::Config::new(size.width, size.height, framerate);
		config.color = color;
		config.kind = listing.kind.clone();
		let rendition = config.probe().await?;
		let reserved = listing.catalog.reserve().with_interval(config.effective_join());
		let suffix = match &rendition.codec {
			hang::catalog::VideoCodec::H264(_) => ".avc3",
			hang::catalog::VideoCodec::H265(_) => ".hev1",
			other => return Err(Error::Unsupported(format!("{other} is not a codec the studio publishes"))),
		};
		let track = listing
			.broadcast
			.unique_track(suffix, reserved.track_info(hang::catalog::PRIORITY.video))?;
		let producer = moq_video::encode::Producer::new(track, reserved, rendition)?;
		Ok(Self {
			track: Encoding::new(producer, listing.sessions.clone()),
			claim: listing.claim,
			grants: listing.grants.clone(),
			granted: None,
			on_program: listing.on_program.clone(),
			config,
			clock: listing.studio.clock(),
			epoch: None,
		})
	}

	/// Start the private timeline at `origin` on the studio clock, rather than
	/// where the first frame encodes, for a source that encodes its pictures
	/// ahead of when they show.
	pub(crate) fn anchor(&mut self, origin: kio::clock::Instant) -> Result<(), Error> {
		self.epoch = Some(self.clock.capture(origin)?.as_micros());
		Ok(())
	}

	/// Restamp `frame` onto the program's clock, and encode and publish it
	/// while the source is used: shown on program, or its track read. The
	/// encoder opens on the first frame used and closes while none is, marking
	/// the gap, and reopens when the picture's size or color space changes,
	/// which moves the rendition to a new track.
	pub(crate) fn encode(&mut self, frame: &mut moq_video::Frame) -> Result<(), Error> {
		let captured = frame.timestamp.as_micros();
		let epoch = *self
			.epoch
			.get_or_insert_with(|| self.clock.now().as_micros().saturating_sub(captured));
		frame.timestamp = moq_net::Timestamp::from_micros((epoch + captured) as u64).map_err(moq_video::Error::from)?;

		if !self.used() {
			return self.release();
		}

		let size = encodable(frame.size());
		let color = frame.surface.color();
		if size != self.config.size() || color.is_some_and(|color| Some(color) != self.config.color) {
			self.config.width = size.width;
			self.config.height = size.height;
			self.config.color = color;
			// Before the next one opens, so a GPU at its session cap has room.
			self.track.drain()?;
		}
		if !self.track.is_open() {
			self.track.open(moq_video::encode::Encoder::new(&self.config)?)?;
			self.granted = None;
		}
		// Published independently, it encodes at what the connection grants.
		let grant = self
			.grants
			.lock()
			.unwrap()
			.get(&self.claim.name)
			.and_then(|(_, grant)| grant.peek());
		if let Some(rate) = grant
			&& self.granted != Some(rate)
		{
			self.granted = Some(rate);
			match self.track.set_bitrate(rate) {
				Ok(()) => {}
				// Not fatal: it keeps encoding at its ceiling, and only stops
				// following the grant, as the program does.
				Err(Error::Video(moq_video::Error::BitrateUnsupported(name))) => {
					tracing::warn!(source = %self.claim.name, encoder = name, "the encoder cannot follow the bandwidth grant");
				}
				Err(err) => return Err(err),
			}
		}
		if frame.size() == size {
			self.track.encode(frame)
		} else {
			self.track.encode(&frame.resize(size, &Default::default())?)
		}
	}

	/// Whether the source is used: shown on program, or its track read.
	fn used(&self) -> bool {
		self.on_program.borrow().shown.contains(&self.claim.name) || self.track.watched()
	}

	/// Wait until whether the source is used disagrees with whether its
	/// encoder is open, so a source that delivers nothing new, such as a
	/// still screen or a media file that ended, can act without waiting on a
	/// picture: [`release`] the encoder when it is open and nothing uses the
	/// source, or bring a picture when it is closed and the source goes into
	/// use.
	///
	/// [`release`]: Self::release
	pub(crate) async fn settle(&self) {
		let mut demand = self.track.demand();
		let mut on_program = self.on_program.clone();
		on_program.mark_unchanged();
		while self.used() == self.track.is_open() {
			// The first look at the demand reports it as it is now.
			tokio::select! {
				changed = on_program.changed() => if changed.is_err() { std::future::pending::<()>().await },
				wanted = demand.wanted_changed() => if wanted.is_err() { std::future::pending::<()>().await },
			}
		}
	}

	/// Whether the encoder is open.
	pub(crate) fn is_open(&self) -> bool {
		self.track.is_open()
	}

	/// Take pictures from a reopened capture, whose private timeline starts
	/// over: the next picture maps it onto the program's clock afresh.
	#[cfg_attr(
		not(any(feature = "capture", test)),
		expect(dead_code, reason = "only a capture reopens")
	)]
	pub(crate) fn reopened(&mut self) {
		self.epoch = None;
	}

	/// Close the encoder while nothing uses the source, marking the gap, which
	/// gives the encode session back.
	pub(crate) fn release(&mut self) -> Result<(), Error> {
		if self.track.is_open() && !self.used() {
			self.track.discontinuity()?;
		}
		Ok(())
	}

	/// Start a group with the next picture, as a keyframe, such as where a
	/// media file plays again from its start. A closed encoder starts with one
	/// anyway.
	pub(crate) fn cut(&mut self) -> Result<(), Error> {
		if self.track.is_open() {
			self.track.cut()?;
		}
		Ok(())
	}

	/// Mark a break, such as a gap in a replay: what follows does not continue
	/// what came before, and a viewer arriving during it waits for what follows
	/// rather than being served the last picture as if it were live.
	pub(crate) fn discontinuity(&mut self) -> Result<(), Error> {
		self.track.discontinuity()
	}

	/// End the track, failed with `err` so viewers see why.
	pub(crate) fn abort(self, err: &Error) {
		self.track.abort(moq_net::Error::Transport(err.to_string()));
	}
}

/// The size an encoder takes for a picture of `size`: even, since the encoder
/// subsamples chroma. An odd picture is scaled down to it.
fn encodable(size: moq_video::Size) -> moq_video::Size {
	moq_video::Size::new((size.width & !1).max(2), (size.height & !1).max(2))
}

/// A captured or played source's sound, encoded as Opus while the source is
/// used, as captured or decoded: before the gain and mute the program mix
/// applies to it.
pub(crate) struct Audio<'a> {
	/// Taken only as the track ends.
	producer: Option<moq_audio::encode::Producer>,
	claim: &'a Claim,
	/// The sources on program.
	on_program: tokio::sync::watch::Receiver<OnProgram>,
	clock: moq_mux::Clock,
	/// Whether sound went to the encoder since the source last went out of
	/// use, so leaving use marks one gap.
	encoding: bool,
}

impl<'a> Audio<'a> {
	/// Advertise an Opus rendition of interleaved `f32` sound in `input`'s
	/// sample rate and layout.
	pub(crate) fn new(listing: &Listing<'a>, input: moq_audio::encode::Input) -> Result<Self, Error> {
		let mut options = moq_audio::encode::Options::default();
		options.config = moq_audio::encode::Config::from_input(moq_audio::encode::Codec::Opus, &input);
		let mut broadcast = listing.broadcast.clone();
		let producer = moq_audio::encode::Producer::new(&mut broadcast, listing.catalog.clone(), input, &options)?;
		Ok(Self {
			producer: Some(producer),
			claim: listing.claim,
			on_program: listing.on_program.clone(),
			clock: listing.studio.clock(),
			encoding: false,
		})
	}

	/// Encode and publish `samples`, whose first was captured or sounds at
	/// `captured`, while the source is used: heard on program, or its track
	/// read. `gap` says sound was lost just before them. Leaving use marks a
	/// gap, and the sound after one is stamped afresh, so the track's timeline
	/// holds it.
	pub(crate) fn write(&mut self, samples: &[f32], captured: kio::clock::Instant, gap: bool) -> Result<(), Error> {
		let producer = self.producer.as_mut().expect("open until it ends");
		let used = self.on_program.borrow().heard.contains(&self.claim.name) || producer.demand().wanted().is_some();
		if !used {
			if std::mem::take(&mut self.encoding) {
				producer.discontinuity()?;
			}
			return Ok(());
		}
		if gap && self.encoding {
			producer.reanchor();
		}
		self.encoding = true;
		// A media source writes its sound ahead of when it sounds, and a device
		// may stamp a buffer a moment ahead of the clock, which maps only what
		// has passed: the rest is counted on from now.
		let now = kio::clock::now();
		let ahead = captured.saturating_duration_since(now).as_micros();
		let timestamp = self.clock.capture(captured.min(now))?.as_micros() + ahead;
		let timestamp = moq_net::Timestamp::from_micros(timestamp as u64).map_err(moq_audio::Error::from)?;
		let data = bytes::Bytes::copy_from_slice(bytemuck::cast_slice(samples));
		producer.write(&moq_audio::Frame::new(data, timestamp))?;
		Ok(())
	}

	/// End the track, failed with `err` so viewers see why.
	pub(crate) fn abort(mut self, err: &Error) {
		if let Some(producer) = self.producer.take() {
			producer.abort(moq_net::Error::Transport(err.to_string()));
		}
	}
}

impl Drop for Audio<'_> {
	/// End the track cleanly, after publishing what the encoder still holds,
	/// or failed with the reason that could not be done.
	fn drop(&mut self) {
		if let Some(mut producer) = self.producer.take()
			&& let Err(err) = producer.finish()
		{
			producer.abort(moq_net::Error::Transport(err.to_string()));
		}
	}
}

/// A still picture, published once as the image file it is.
pub(crate) struct Still {
	track: moq_net::track::Producer,
}

impl Still {
	/// Publish `bytes`, a PNG or JPEG file, as a one-group binary track.
	pub(crate) fn new(listing: &Listing<'_>, bytes: bytes::Bytes) -> Result<Self, Error> {
		let format = image::guess_format(&bytes).map_err(|err| Error::Image(err.to_string()))?;
		let suffix = match format {
			image::ImageFormat::Png => ".png",
			image::ImageFormat::Jpeg => ".jpg",
			other => return Err(Error::Image(format!("{other:?} is not a format the studio publishes"))),
		};
		let info = listing.studio.track_info(hang::catalog::PRIORITY.video);
		let mut track = listing.broadcast.unique_track(suffix, info)?;
		let mut config = hang::catalog::BinaryConfig::new(hang::catalog::Mode::Snapshot);
		config.mime = Some(format.to_mime_type().to_string());
		let mut catalog = listing.catalog.clone();
		catalog.mutate(|catalog| {
			catalog.binary.tracks.insert(track.name().to_string(), config);
		})?;
		// The newest group never goes stale, so this one serves every viewer.
		track.write_frame(listing.studio.clock().now(), bytes)?;
		Ok(Self { track })
	}

	/// A solid color as a one-pixel PNG, so every still reads the same way.
	pub(crate) fn color(listing: &Listing<'_>, color: crate::Rgba) -> Result<Self, Error> {
		let pixel = image::RgbaImage::from_raw(1, 1, color.0.to_vec()).expect("one pixel");
		let mut png = std::io::Cursor::new(Vec::new());
		pixel
			.write_to(&mut png, image::ImageFormat::Png)
			.map_err(|err| Error::Image(err.to_string()))?;
		Self::new(listing, png.into_inner().into())
	}
}

impl Drop for Still {
	fn drop(&mut self) {
		let _ = self.track.finish();
	}
}

#[cfg(test)]
mod tests {
	use moq_mux::catalog::Stream;

	use super::*;
	use crate::collection::{Collection, Rgba, Source};
	use crate::source::test::{context, local, start, until, until_time, video};
	use crate::source::{Sources, Status};

	/// Where sources publish, its catalog, and a reader of its broadcast.
	fn studio() -> (Publish, Catalog, moq_net::broadcast::Consumer) {
		let clock = moq_mux::Clock::new();
		let publish = Publish::new(clock, moq_video::encode::Kind::Auto, Sessions::default()).unwrap();
		let (catalog, consumer) = (publish.catalog.clone(), publish.consume());
		(publish, catalog, consumer)
	}

	/// On program with only `name`'s picture shown.
	fn shown(name: &str) -> OnProgram {
		OnProgram {
			shown: [name.to_string()].into(),
			..Default::default()
		}
	}

	/// On program with only `name`'s sound heard.
	fn heard(name: &str) -> OnProgram {
		OnProgram {
			heard: [name.to_string()].into(),
			..Default::default()
		}
	}

	/// Walk a media source playing a 10 fps clip through `count` of its play's
	/// pictures from the one at index `from`, returning when the play began.
	/// The clock is held until the play begins, then moves to each picture's
	/// moment and is held while it decodes, encodes, and shows, so no wait
	/// counts on work in threads the runtime cannot see, such as a decoder's or
	/// an encoder's, finishing within any span of the clock.
	async fn pictures(feed: &crate::source::Feed, from: u32, count: u32) -> kio::clock::Instant {
		let begun = until(|| feed.run().begun).await;
		let shown = || matches!(feed.next_picture(), Some(crate::source::Picture::Frame(_))).then_some(());
		for index in from..from + count {
			until_time(begun + std::time::Duration::from_millis(100) * index).await;
			until(shown).await;
		}
		begun
	}

	/// Yield until `f` holds of the local catalog's source listings.
	async fn listed(catalog: &Catalog, f: impl Fn(&BTreeMap<String, hang::Catalog>) -> bool) -> hang::Catalog<Ext> {
		for _ in 0..1000 {
			let snapshot = catalog.snapshot();
			if f(&snapshot.ext.studio.sources) {
				return snapshot;
			}
			tokio::task::yield_now().await;
		}
		panic!("never listed: {:?}", catalog.snapshot().ext.studio.sources);
	}

	/// The one payload a still's track holds.
	async fn still(consumer: &moq_net::broadcast::Consumer, source: &hang::Catalog) -> (bytes::Bytes, String) {
		let (name, config) = source.binary.tracks.iter().next().expect("a still track");
		let mut track = consumer.track(name).unwrap().subscribe(None).await.unwrap();
		let mut group = track.recv_group().await.unwrap().expect("a group");
		let frame = group.read_frame().await.unwrap().expect("a frame");
		(frame.payload, config.mime.clone().expect("a mime type"))
	}

	/// Stills publish once, as image files listed under their source's name
	/// rather than as renditions of the program, and leave the listing when
	/// removed. A broadcast or a clone publishes nothing of its own.
	#[tokio::test]
	async fn stills_are_listed_by_source() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let mut png = image::RgbaImage::new(3, 2);
			png.put_pixel(2, 1, image::Rgba([1, 2, 3, 4]));
			png.save(dir.path().join("logo.png")).unwrap();

			let (publish, catalog, consumer) = studio();
			let (mut context, _origin) = context(dir.path());
			context.publish = Some(publish);
			let mut collection = Collection::new();
			collection.sources.insert(
				"red".into(),
				Source::Color {
					color: Rgba([255, 0, 0, 255]),
				},
			);
			collection.sources.insert(
				"logo".into(),
				Source::Image {
					path: "logo.png".into(),
				},
			);
			collection.sources.insert(
				"guest".into(),
				Source::Broadcast {
					path: "room/guest".into(),
				},
			);
			collection
				.sources
				.insert("red copy".into(), Source::Clone { source: "red".into() });
			collection.validate().unwrap();
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;

			let snapshot = listed(&catalog, |sources| sources.len() == 2).await;
			assert!(snapshot.video.renditions.is_empty(), "no source is a program rendition");
			let published = &snapshot.ext.studio.sources;
			assert!(published["red"].media.is_empty(), "the root carries the media timing");
			assert_eq!(sources.get("red").unwrap().track(), Some(Status::Live));
			assert_eq!(sources.get("guest").unwrap().track(), None);
			assert!(sources.get("red copy").is_none(), "a clone opens no feed, so no track");

			// A viewer reads the same listing off the wire.
			let format = moq_mux::catalog::CatalogFormat::default();
			let mut wire = moq_mux::catalog::Consumer::<Ext>::new(&consumer, format).await.unwrap();
			loop {
				let read = wire.next().await.unwrap().expect("a catalog");
				if read.ext.studio.sources == *published {
					break;
				}
			}

			let (payload, mime) = still(&consumer, &published["red"]).await;
			assert_eq!(mime, "image/png");
			let red = image::load_from_memory(&payload).unwrap().into_rgba8();
			assert_eq!(red.dimensions(), (1, 1));
			assert_eq!(red.into_raw(), [255, 0, 0, 255]);

			let (payload, mime) = still(&consumer, &published["logo"]).await;
			assert_eq!(mime, "image/png");
			assert_eq!(payload, std::fs::read(dir.path().join("logo.png")).unwrap());

			collection.sources.remove("red copy");
			collection.sources.remove("red");
			sources.sync(&collection);
			let snapshot = listed(&catalog, |sources| !sources.contains_key("red")).await;
			assert!(snapshot.ext.studio.sources.contains_key("logo"));
		})
		.await;
	}

	/// A source published independently is listed in the program's catalog,
	/// its track readable from the program's broadcast under [`FORWARDED`],
	/// for as long as it is published; another stays in the studio.
	#[tokio::test]
	async fn a_published_source_is_forwarded_to_the_program() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (publish, _, _) = studio();
			let (mut context, _origin) = context(dir.path());
			context.publish = Some(publish.clone());
			let mut collection = Collection::new();
			for (name, color) in [("red", [255, 0, 0, 255]), ("blue", [0, 0, 255, 255])] {
				collection
					.sources
					.insert(name.into(), Source::Color { color: Rgba(color) });
			}
			collection.published.insert("red".into());
			collection.validate().unwrap();
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;

			let mut program = moq_net::broadcast::Info::new().produce();
			let config = moq_mux::catalog::Config::default().with_catalog(Default::default());
			let catalog = Catalog::new(&mut program, config).unwrap();
			let (published, follow) = tokio::sync::watch::channel(collection.published.clone());
			let forward = publish.clone();
			let (broadcast, listing) = (program.clone(), catalog.clone());
			tokio::spawn(async move {
				forward
					.forward(follow, broadcast, listing, moq_net::bandwidth::Allocator::unlimited())
					.await
					.unwrap()
			});

			let snapshot = listed(&catalog, |sources| sources.contains_key("red")).await;
			let red = &snapshot.ext.studio.sources["red"];
			assert!(
				!snapshot.ext.studio.sources.contains_key("blue"),
				"only what is published"
			);
			assert!(red.binary.tracks.keys().all(|name| name.starts_with(FORWARDED)));
			let (payload, mime) = still(&program.consume(), red).await;
			assert_eq!(mime, "image/png");
			let red = image::load_from_memory(&payload).unwrap().into_rgba8();
			assert_eq!(red.into_raw(), [255, 0, 0, 255]);

			published.send(BTreeSet::new()).unwrap();
			listed(&catalog, |sources| sources.is_empty()).await;
		})
		.await;
	}

	/// Stopping the sources, as quitting does, ends each source's own track
	/// cleanly after what it published, and takes it out of the listing.
	#[tokio::test]
	async fn stopping_the_sources_finishes_their_tracks() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (publish, catalog, consumer) = studio();
			let (mut context, _origin) = context(dir.path());
			context.publish = Some(publish);
			let mut collection = Collection::new();
			collection.sources.insert(
				"red".into(),
				Source::Color {
					color: Rgba([255, 0, 0, 255]),
				},
			);
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			let snapshot = listed(&catalog, |sources| sources.contains_key("red")).await;
			let (name, _) = snapshot.ext.studio.sources["red"].binary.tracks.iter().next().unwrap();
			let mut track = consumer.track(name).unwrap().subscribe(None).await.unwrap();

			sources.stop();
			assert!(sources.get("red").is_none());
			let mut group = track.recv_group().await.unwrap().expect("the still");
			assert!(group.read_frame().await.unwrap().is_some());
			assert!(group.read_frame().await.unwrap().is_none());
			assert!(track.recv_group().await.unwrap().is_none(), "the track finished");
			listed(&catalog, |sources| sources.is_empty()).await;
		})
		.await;
	}

	/// Stopping the sources ends a playing media file's picture and sound
	/// tracks cleanly after what they published.
	#[tokio::test(start_paused = true)]
	async fn stopping_the_sources_finishes_a_media_files_tracks() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			std::fs::write(dir.path().join("bed.mkv"), crate::media::tests::CLIP).unwrap();
			let (publish, catalog, consumer) = studio();
			publish.set_on_program(OnProgram {
				shown: ["bed".to_string()].into(),
				heard: ["bed".to_string()].into(),
			});
			let (mut context, _origin) = context(dir.path());
			context.publish = Some(publish.clone());
			let mut collection = Collection::new();
			let bed = Source::Media {
				path: "bed.mkv".into(),
				looping: true,
				restart: false,
			};
			collection.sources.insert("bed".into(), bed);
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;

			let bed = until(|| {
				let listed = catalog.snapshot().ext.studio.sources.remove("bed");
				listed.filter(|listed| !listed.video.renditions.is_empty() && !listed.audio.renditions.is_empty())
			})
			.await;
			let names = [
				bed.video.renditions.keys().next().unwrap().clone(),
				bed.audio.renditions.keys().next().unwrap().clone(),
			];
			let mut tracks = Vec::new();
			for name in &names {
				let subscription =
					moq_net::track::Subscription::default().with_delay(moq_net::track::Delay::default().with_max(None));
				tracks.push(consumer.track(name).unwrap().subscribe(subscription).await.unwrap());
			}
			let written = |id: &str| publish.stats().get(id).map_or(0, |rendition| rendition.frames);
			pictures(sources.get("bed").unwrap(), 0, 4).await;
			until(|| names.iter().all(|name| written(name) > 0).then_some(())).await;

			sources.stop();
			for (name, mut track) in names.iter().zip(tracks) {
				let mut frames = 0;
				while let Some(mut group) = track.recv_group().await.expect("a clean end") {
					while group.read_frame().await.expect("a whole group").is_some() {
						frames += 1;
					}
				}
				assert!(frames > 0, "{name} published nothing");
			}
			listed(&catalog, |sources| sources.is_empty()).await;
		})
		.await;
	}

	/// A mid-gray picture of `size` at `millis` on a capture's own timeline.
	fn gray(size: moq_video::Size, millis: u64) -> moq_video::Frame {
		let i420 = moq_video::I420::new(size, vec![0x80; moq_video::I420::len(size).unwrap()]).unwrap();
		let timestamp = moq_net::Timestamp::from_micros(millis * 1000).unwrap();
		moq_video::Frame::new(moq_video::Surface::I420(i420), timestamp)
	}

	/// A captured picture encodes into its source's own listing only while the
	/// source is used, shown on program or its track read, holding an encode
	/// session only then, and one that changes size reopens the encoder rather
	/// than failing the track.
	#[tokio::test]
	async fn a_video_encodes_while_used() {
		local(async {
			let (publish, catalog, consumer) = studio();
			let claim = publish.claim("camera");
			let listing = Listing::new(&claim, &publish).unwrap();
			let framerate = moq_video::FrameRate::new(30, 1).unwrap();
			let width = |sources: &BTreeMap<String, hang::Catalog>| {
				let source = sources.get("camera")?;
				let mut renditions = source.video.renditions.values();
				let width = renditions.next()?.coded_width;
				renditions.next().is_none().then_some(width).flatten()
			};
			let written = || publish.stats().values().map(|rendition| rendition.frames).sum::<u64>();

			tokio::select! {
				result = listing.mirror() => panic!("the mirror ended: {result:?}"),
				() = async {
					let small = moq_video::Size::new(64, 48);
					let mut video = Video::new(&listing, small, framerate, None).await.unwrap();
					for millis in [0, 33] {
						video.encode(&mut gray(small, millis)).unwrap();
					}
					assert_eq!((written(), publish.sessions.open()), (0, 0), "unused");
					let snapshot = listed(&catalog, |sources| width(sources) == Some(64)).await;
					assert!(snapshot.video.renditions.is_empty(), "no source is a program rendition");

					publish.set_on_program(shown("camera"));
					for millis in [66, 100, 133] {
						video.encode(&mut gray(small, millis)).unwrap();
					}
					assert_eq!((written(), publish.sessions.open()), (3, 1), "on program");

					let large = moq_video::Size::new(80, 60);
					for millis in [166, 200] {
						video.encode(&mut gray(large, millis)).unwrap();
					}
					let snapshot = listed(&catalog, |sources| width(sources) == Some(80)).await;
					assert_eq!(publish.sessions.open(), 1, "the resize closed the old encoder");

					publish.set_on_program(Default::default());
					video.encode(&mut gray(large, 233)).unwrap();
					assert_eq!((written(), publish.sessions.open()), (5, 0), "off program");

					// A reader, such as a recording, uses it too.
					let name = snapshot.ext.studio.sources["camera"].video.renditions.keys().next().unwrap();
					let mut track = consumer.track(name).unwrap().subscribe(None).await.unwrap();
					video.encode(&mut gray(large, 266)).unwrap();
					assert_eq!((written(), publish.sessions.open()), (6, 1), "read");

					// Removing the source ends its track cleanly.
					drop(video);
					while let Some(mut group) = track.recv_group().await.unwrap() {
						while group.read_frame().await.unwrap().is_some() {}
					}
					assert_eq!(publish.sessions.open(), 0);
				} => {}
			}

			// Every frame written counts for the Health panel, while the
			// local catalog lists the source.
			assert_eq!(written(), 6);
			// A replacement takes the listing, and the old tracks with it.
			let _newer = publish.claim("camera");
			assert!(publish.stats().is_empty());
			drop(listing);
			assert!(publish.owners.lock().unwrap().catalogs.is_empty());
		})
		.await;
	}

	/// A source leaving program gives its encode session back without waiting
	/// on a picture, since a still screen delivers none, and one going on
	/// program while closed says so, so its capture can bring a picture.
	#[tokio::test(start_paused = true)]
	async fn a_video_follows_its_use_without_a_picture() {
		local(async {
			let (publish, _catalog, _consumer) = studio();
			let claim = publish.claim("display");
			let listing = Listing::new(&claim, &publish).unwrap();
			let framerate = moq_video::FrameRate::new(30, 1).unwrap();
			let size = moq_video::Size::new(64, 48);
			let mut video = Video::new(&listing, size, framerate, None).await.unwrap();
			let wait = std::time::Duration::from_secs(1);

			assert!(
				kio::clock::timeout(wait, video.settle()).await.is_err(),
				"unused and closed"
			);
			publish.set_on_program(shown("display"));
			kio::clock::timeout(wait, video.settle())
				.await
				.expect("on program without a picture, so the capture brings one");
			video.encode(&mut gray(size, 0)).unwrap();
			assert_eq!(publish.sessions.open(), 1);
			assert!(kio::clock::timeout(wait, video.settle()).await.is_err(), "on program");

			publish.set_on_program(Default::default());
			kio::clock::timeout(wait, video.settle()).await.expect("off program");
			video.release().unwrap();
			assert_eq!(publish.sessions.open(), 0);
			assert!(kio::clock::timeout(wait, video.settle()).await.is_err(), "closed");

			// A capture reopened a minute on starts its own timeline over, which
			// lands on the program's clock now rather than a minute back.
			kio::clock::sleep(std::time::Duration::from_secs(60)).await;
			video.reopened();
			publish.set_on_program(shown("display"));
			let now = publish.catalog.clock().now();
			let mut picture = gray(size, 0);
			video.encode(&mut picture).unwrap();
			assert!(picture.timestamp >= now, "{:?} before {now:?}", picture.timestamp);
		})
		.await;
	}

	/// A source off program encodes only while its track is subscribed, such
	/// as by a recording, holding no encoder between subscribers.
	#[tokio::test]
	async fn an_unshown_video_encodes_while_subscribed() {
		local(async {
			let (publish, catalog, consumer) = studio();
			let claim = publish.claim("camera");
			let listing = Listing::new(&claim, &publish).unwrap();
			let framerate = moq_video::FrameRate::new(30, 1).unwrap();
			let size = moq_video::Size::new(64, 48);

			tokio::select! {
				result = listing.mirror() => panic!("the mirror ended: {result:?}"),
				() = async {
					let mut video = Video::new(&listing, size, framerate, None).await.unwrap();
					video.encode(&mut gray(size, 0)).unwrap();
					assert!(!video.track.is_open(), "nothing encodes unsubscribed");

					// Advertised before anything encodes, so a recording can find it.
					let rendition = |sources: &BTreeMap<String, hang::Catalog>| {
						sources.get("camera").is_some_and(|camera| !camera.video.renditions.is_empty())
					};
					let snapshot = listed(&catalog, rendition).await;
					let name = snapshot.ext.studio.sources["camera"].video.renditions.keys().next().unwrap().clone();
					let mut track = consumer.track(&name).unwrap().subscribe(None).await.unwrap();
					video.encode(&mut gray(size, 33)).unwrap();
					assert!(video.track.is_open(), "a subscriber gets pictures");
					let mut group = track.recv_group().await.unwrap().expect("a group");
					assert!(group.read_frame().await.unwrap().is_some());
					let first = group.sequence;

					drop(group);
					drop(track);
					for _ in 0..100 {
						tokio::task::yield_now().await;
					}
					video.encode(&mut gray(size, 66)).unwrap();
					assert!(!video.track.is_open(), "the encoder closes once nobody subscribes");

					// The next subscriber gets a fresh encoder's pictures, in a group
					// after the gap rather than the one before it.
					let mut track = consumer.track(&name).unwrap().subscribe(None).await.unwrap();
					video.encode(&mut gray(size, 100)).unwrap();
					assert!(video.track.is_open(), "a new subscriber reopens the encoder");
					let mut group = track.recv_group().await.unwrap().expect("a group");
					assert!(group.sequence > first, "{} after {first}", group.sequence);
					assert!(group.read_frame().await.unwrap().is_some());
				} => {}
			}
		})
		.await;
	}

	/// Ten milliseconds of a quiet 440 Hz tone, in stereo at 48 kHz.
	fn tone() -> Vec<f32> {
		(0..480)
			.flat_map(|index| {
				let sample = (index as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.25;
				[sample, sample]
			})
			.collect()
	}

	/// What a viewer of a finished track reads, in order: each packet's
	/// timestamp, and `None` where the media stops for a gap.
	async fn packets(
		broadcast: &moq_net::broadcast::Consumer,
		name: &str,
		config: &hang::catalog::AudioConfig,
	) -> Vec<Option<moq_net::Timestamp>> {
		let subscription =
			moq_net::track::Subscription::default().with_delay(moq_net::track::Delay::default().with_max(None));
		let track = broadcast
			.track(hang::catalog::Reference::track(config, name))
			.unwrap()
			.subscribe(subscription)
			.await
			.unwrap();
		let container = moq_mux::catalog::hang::Container::try_from(config).unwrap();
		let mut reader = moq_mux::container::Consumer::new(track, container);
		let mut read = Vec::new();
		while let Some(next) = reader.read().await.unwrap() {
			read.push(match next {
				moq_mux::container::Read::Frame { frame, .. } => Some(frame.timestamp),
				moq_mux::container::Read::End { .. } | moq_mux::container::Read::Gap { .. } => None,
			});
		}
		read
	}

	/// A microphone's sound encodes into its own listing only while the
	/// source is used, heard on program or its track read, stamped on the
	/// program's clock as it was captured. A gap in the capture, or the
	/// source leaving use, is a gap on the track too, and a replay of it holds
	/// what was encoded, refusing a span with nothing in it.
	#[tokio::test(start_paused = true)]
	async fn an_audio_encodes_while_used() {
		local(async {
			let (publish, catalog, consumer) = studio();
			let claim = publish.claim("mic");
			let listing = Listing::new(&claim, &publish).unwrap();
			let clock = publish.catalog.clock();
			let written = || publish.stats().values().map(|rendition| rendition.frames).sum::<u64>();
			let samples = tone();
			let tick = std::time::Duration::from_millis(10);

			tokio::select! {
				result = listing.mirror() => panic!("the mirror ended: {result:?}"),
				() = async {
					let input = moq_audio::encode::Input::new(48_000, moq_audio::Layout::Stereo);
					let mut audio = Audio::new(&listing, input).unwrap();
					// A tenth of a second, captured as the clock goes.
					let capture = async |audio: &mut Audio<'_>, gap: bool| {
						for index in 0..10 {
							audio.write(&samples, kio::clock::now(), gap && index == 0).unwrap();
							kio::clock::sleep(tick).await;
						}
					};

					// Advertised before anything encodes, so a recording can find it.
					let rendition = |sources: &BTreeMap<String, hang::Catalog>| {
						sources.get("mic").is_some_and(|mic| !mic.audio.renditions.is_empty())
					};
					let snapshot = listed(&catalog, rendition).await;
					let (name, config) = snapshot.ext.studio.sources["mic"].audio.renditions.first_key_value().unwrap();
					assert_eq!(config.codec, hang::catalog::AudioCodec::Opus);
					assert!(snapshot.audio.renditions.is_empty(), "no source is a program rendition");
					let target = || crate::replay::Target {
						broadcast: consumer.clone(),
						video: None,
						audio: Some((name.clone(), config.clone())),
						end: clock.now(),
					};

					capture(&mut audio, false).await;
					assert_eq!(written(), 0, "unused");
					let refused = crate::replay::Take::new(&target(), std::time::Duration::from_secs(1));
					assert!(refused.is_err(), "nothing to replay");

					publish.set_on_program(heard("mic"));
					let heard = clock.now();
					capture(&mut audio, false).await;
					assert!(written() > 0, "on program");
					// The capture lost half a second, which the track keeps.
					kio::clock::sleep(std::time::Duration::from_millis(500)).await;
					let resumed = clock.now();
					capture(&mut audio, true).await;
					let take = crate::replay::Take::new(&target(), std::time::Duration::from_secs(5)).unwrap();

					// Off program, past the gap the first buffer marks.
					publish.set_on_program(Default::default());
					capture(&mut audio, false).await;
					let before = written();
					capture(&mut audio, false).await;
					assert_eq!(written(), before, "off program");

					// A reader, such as a recording, uses it too.
					let mut track = consumer.track(name).unwrap().subscribe(None).await.unwrap();
					capture(&mut audio, false).await;
					assert!(written() > before, "read");

					// Removing the source ends its track cleanly.
					drop(audio);
					while let Some(mut group) = track.recv_group().await.unwrap() {
						while group.read_frame().await.unwrap().is_some() {}
					}

					// The replay holds the sound from where the source went on
					// program, a codec frame of priming aside, with the lost half
					// second kept as a gap.
					let (_, config) = take.audio.as_ref().unwrap();
					let read = packets(&take.consume(), name, config).await;
					let near = |at: moq_net::Timestamp, to: moq_net::Timestamp| {
						at.as_micros().abs_diff(to.as_micros()) <= 20_000
					};
					let first = read.first().copied().flatten().expect("a packet first");
					assert!(near(first, heard), "{first:?} for {heard:?}");
					let gap = read.iter().position(Option::is_none).expect("a gap");
					let after = read[gap + 1].expect("a packet after the gap");
					assert!(near(after, resumed), "{after:?} for {resumed:?}");
					assert!(read[..gap].iter().all(|at| at.is_some_and(|at| at < resumed)));
				} => {}
			}
		})
		.await;
	}

	/// Holds its newest pictures back the way a pipelined hardware encoder
	/// does, handing them over only on a flush or finish. openh264, the encoder
	/// on every machine, holds nothing.
	struct Pipelined {
		encoder: moq_video::encode::Encoder,
		held: Vec<moq_video::encode::Encoded>,
	}

	impl Pipelined {
		const DEPTH: usize = 2;
	}

	impl Encode for Pipelined {
		fn encode(&mut self, frame: &moq_video::Frame) -> Result<Vec<moq_video::encode::Encoded>, moq_video::Error> {
			self.held.extend(self.encoder.encode(frame)?);
			let ready = self.held.len().saturating_sub(Self::DEPTH);
			Ok(self.held.drain(..ready).collect())
		}

		fn flush(&mut self) -> Result<Vec<moq_video::encode::Encoded>, moq_video::Error> {
			let mut encoded = std::mem::take(&mut self.held);
			encoded.extend(self.encoder.flush()?);
			Ok(encoded)
		}

		fn finish(mut self) -> Result<Vec<moq_video::encode::Encoded>, moq_video::Error> {
			let mut encoded = std::mem::take(&mut self.held);
			encoded.extend(self.encoder.finish()?);
			Ok(encoded)
		}

		fn cut(&mut self) -> Result<(), moq_video::Error> {
			self.encoder.cut()
		}

		fn set_bitrate(&mut self, bitrate: moq_net::bandwidth::Rate) -> Result<(), moq_video::Error> {
			self.encoder.set_bitrate(bitrate)
		}
	}

	/// A pipelined encoder's last pictures land before a pause's gap rather
	/// than after it, the picture after the gap opens a group of its own, and
	/// the pictures it still holds as the track ends are published before the
	/// track finishes cleanly.
	#[tokio::test]
	async fn a_pipelined_encoder_loses_no_pictures() {
		let (publish, _catalog, consumer) = studio();
		let claim = publish.claim("program");
		let listing = Listing::new(&claim, &publish).unwrap();
		let size = moq_video::Size::new(64, 48);
		let framerate = moq_video::FrameRate::new(30, 1).unwrap();
		let mut config = moq_video::encode::Config::new(size.width, size.height, framerate);
		config.kind = moq_video::encode::Kind::Named("openh264".into());
		let rendition = config.probe().await.unwrap();
		let reserved = listing.catalog.reserve();
		let track = listing
			.broadcast
			.unique_track(".avc3", reserved.track_info(hang::catalog::PRIORITY.video))
			.unwrap();
		let producer = moq_video::encode::Producer::new(track, reserved, rendition).unwrap();
		let (name, rendition) = listing.catalog.snapshot().video.renditions.pop_first().unwrap();
		// Every group, however far behind the newest, since this reads only once
		// the track has ended.
		let subscription =
			moq_net::track::Subscription::default().with_delay(moq_net::track::Delay::default().with_max(None));
		let subscriber = consumer.track(&name).unwrap().subscribe(subscription).await.unwrap();
		let container = moq_mux::catalog::hang::Container::try_from(&rendition).unwrap();
		let mut reader = moq_mux::container::Consumer::new(subscriber, container);

		let mut track = Encoding::new(producer, Sessions::default());
		let encoder = moq_video::encode::Encoder::new(&config).unwrap();
		track
			.open(Pipelined {
				encoder,
				held: Vec::new(),
			})
			.unwrap();
		for millis in [0, 33, 66] {
			track.encode(&gray(size, millis)).unwrap();
		}
		track.pause().unwrap();
		for millis in [500, 533, 566] {
			track.encode(&gray(size, millis)).unwrap();
		}
		track.finish().unwrap();

		/// What a viewer reads, in order.
		#[derive(Debug, PartialEq)]
		enum Seen {
			/// A group opens.
			Group,
			/// A picture, at its timestamp in milliseconds.
			Picture(u128),
			/// The media stops for a gap.
			End,
		}
		let mut seen = Vec::new();
		let mut current = None;
		while let Some(read) = reader.read().await.unwrap() {
			match read {
				moq_mux::container::Read::Frame { group, frame } => {
					if current.replace(group) != Some(group) {
						seen.push(Seen::Group);
					}
					seen.push(Seen::Picture(frame.timestamp.as_micros() / 1000));
				}
				moq_mux::container::Read::End { .. } => seen.push(Seen::End),
				moq_mux::container::Read::Gap { .. } => panic!("pictures went missing"),
			}
		}
		use Seen::*;
		assert_eq!(
			seen,
			[
				Group,
				Picture(0),
				Picture(33),
				Picture(66),
				End,
				Group,
				Picture(500),
				Picture(533),
				Picture(566),
			]
		);
	}

	/// An edited source's replacement owns the listing from the moment it
	/// opens, even when the old source's driver only gets to list itself later,
	/// and the old one's end leaves the replacement listed.
	#[tokio::test]
	async fn a_replaced_source_keeps_its_listing() {
		local(async {
			let (publish, catalog, _consumer) = studio();
			let old_claim = publish.claim("logo");
			let new_claim = publish.claim("logo");
			let new = Listing::new(&new_claim, &publish).unwrap();
			let still = Still::color(&new, Rgba([255, 0, 0, 255])).unwrap();
			let name = still.track.name().to_string();
			let replaced = |sources: &BTreeMap<String, hang::Catalog>| {
				sources
					.get("logo")
					.is_some_and(|logo| logo.binary.tracks.contains_key(&name))
			};
			let old = Listing::new(&old_claim, &publish).unwrap();
			let old_still = Still::color(&old, Rgba([0, 0, 0, 255])).unwrap();

			tokio::select! {
				result = old.mirror() => panic!("the mirror ended: {result:?}"),
				result = new.mirror() => panic!("the mirror ended: {result:?}"),
				() = async {
					listed(&catalog, replaced).await;
					// Let the old mirror write too, if it would.
					for _ in 0..100 {
						tokio::task::yield_now().await;
					}
				} => {}
			}
			drop(old_still);
			drop(old);
			drop(old_claim);
			assert!(
				replaced(&catalog.snapshot().ext.studio.sources),
				"the replacement stays listed"
			);

			drop(still);
			drop(new);
			assert!(catalog.snapshot().ext.studio.sources.is_empty());
		})
		.await;
	}

	/// A replay source on program encodes each replay onto its own track as it
	/// shows, gaps included.
	#[tokio::test(start_paused = true)]
	async fn a_replay_is_published_as_it_plays() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			let (publish, catalog, consumer) = studio();
			// On program, so its track encodes.
			publish.set_on_program(shown("replay"));
			let (mut context, _origin) = context(dir.path());
			context.publish = Some(publish);
			let mut collection = Collection::new();
			collection.sources.insert("replay".into(), Source::Replay);
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;

			// The broadcast left out its second second, a gap in the span.
			let (broadcast, video, _live) = video(Some(1)).await;
			let target = crate::replay::Target {
				broadcast: broadcast.consume(),
				video: Some(video),
				audio: None,
				end: moq_net::Timestamp::from_micros(2_500_000).unwrap(),
			};
			let take = crate::replay::Take::new(&target, std::time::Duration::from_millis(2_500)).unwrap();
			let feed = sources.get("replay").unwrap();
			feed.replay(take, 1.0).unwrap();
			// The track opens before the replay begins, so the clock stays put
			// while the encoder opens.
			let listed = until(|| {
				let listed = catalog.snapshot().ext.studio.sources.remove("replay");
				listed.filter(|listed| !listed.video.renditions.is_empty())
			})
			.await;
			assert_eq!(feed.track(), Some(Status::Live));

			// Into the gap, before the replay ends, a viewer reads the break.
			let (name, config) = listed.video.renditions.iter().next().unwrap();
			let track = consumer
				.track(hang::catalog::Reference::track(config, name))
				.unwrap()
				.subscribe(
					moq_net::track::Subscription::default().with_delay(moq_net::track::Delay::default().with_max(None)),
				)
				.await
				.unwrap();
			let container = moq_mux::catalog::hang::Container::try_from(config).unwrap();
			let mut track = moq_mux::container::Consumer::new(track, container);
			let begin = until(|| feed.run().begun).await;
			until_time(begin + std::time::Duration::from_millis(1_500)).await;
			let now = kio::Waiter::noop();
			until(|| match track.poll_read(&now) {
				std::task::Poll::Ready(read) => match read.unwrap().expect("open while the replay plays") {
					moq_mux::container::Read::End { .. } | moq_mux::container::Read::Gap { .. } => Some(()),
					_ => None,
				},
				std::task::Poll::Pending => None,
			})
			.await;
		})
		.await;
	}

	/// A media source on program encodes its pictures onto its own track as
	/// they show, on one timeline when it plays again from its start.
	#[tokio::test(start_paused = true)]
	async fn a_media_file_is_published_on_one_timeline() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			std::fs::write(dir.path().join("clip.mkv"), crate::media::tests::CLIP).unwrap();
			let (publish, catalog, consumer) = studio();
			publish.set_on_program(shown("clip"));
			let sessions = publish.sessions.clone();
			let (mut context, _origin) = context(dir.path());
			context.publish = Some(publish.clone());
			let mut collection = Collection::new();
			let clip = Source::Media {
				path: "clip.mkv".into(),
				looping: false,
				restart: false,
			};
			collection.sources.insert("clip".into(), clip);
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;

			// The pictures' track opens before the play begins, so the clock
			// stays put while the encoder opens.
			let listed = until(|| {
				let listed = catalog.snapshot().ext.studio.sources.remove("clip");
				listed.filter(|listed| !listed.video.renditions.is_empty())
			})
			.await;
			let feed = sources.get("clip").unwrap();
			assert_eq!(feed.track(), Some(Status::Live));
			let (name, config) = listed.video.renditions.iter().next().unwrap();
			let track = consumer
				.track(hang::catalog::Reference::track(config, name))
				.unwrap()
				.subscribe(
					moq_net::track::Subscription::default().with_delay(moq_net::track::Delay::default().with_max(None)),
				)
				.await
				.unwrap();
			let container = moq_mux::catalog::hang::Container::try_from(config).unwrap();
			let mut track = moq_mux::container::Consumer::new(track, container);

			// Half way, five pictures in, it starts over, and plays to its end.
			let first = pictures(feed, 0, 5).await;
			until_time(first + std::time::Duration::from_millis(450)).await;
			let cut = publish.catalog.clock().now();
			feed.play().unwrap();
			let restarted = pictures(feed, 0, 10).await;
			until_time(restarted + std::time::Duration::from_millis(1_100)).await;
			until(|| (feed.status() == Status::Ended).then_some(())).await;

			// How many pictures land depends on the encoder, which may skip
			// some to hold its rate, as openh264 does at this size; the
			// keyframe each play opens with always does.
			let now = kio::Waiter::noop();
			let mut stamps = Vec::new();
			until(|| {
				while let std::task::Poll::Ready(read) = track.poll_read(&now) {
					if let moq_mux::container::Read::Frame { frame, .. } = read.unwrap().expect("open") {
						stamps.push(frame.timestamp);
					}
				}
				stamps.iter().any(|stamp| *stamp >= cut).then_some(())
			})
			.await;
			assert!(stamps[0] < cut, "the first play's keyframe: {stamps:?}");
			assert!(stamps.windows(2).all(|pair| pair[0] < pair[1]), "{stamps:?}");
			// Stamped with when it showed, though encoded ahead of it.
			let shown = publish.catalog.clock().capture(first).unwrap();
			assert!(
				stamps[0].as_micros().abs_diff(shown.as_micros()) <= 1,
				"the first picture as it showed: {:?}, not {shown:?}",
				stamps[0]
			);

			// Ended, it shows nothing new, yet gives its session back once off
			// program and unread.
			assert_eq!(sessions.open(), 1, "on program");
			drop(track);
			publish.set_on_program(Default::default());
			until(|| (sessions.open() == 0).then_some(())).await;
		})
		.await;
	}

	/// A music bed the program mix plays encodes its sound, while its
	/// pictures, which no scene on air shows, encode nothing until one does.
	#[tokio::test(start_paused = true)]
	async fn a_heard_media_file_encodes_only_its_sound() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			std::fs::write(dir.path().join("bed.mkv"), crate::media::tests::CLIP).unwrap();
			let (publish, catalog, _consumer) = studio();
			publish.set_on_program(heard("bed"));
			let sessions = publish.sessions.clone();
			let (mut context, _origin) = context(dir.path());
			context.publish = Some(publish.clone());
			let mut collection = Collection::new();
			let bed = Source::Media {
				path: "bed.mkv".into(),
				looping: true,
				restart: false,
			};
			collection.sources.insert("bed".into(), bed);
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;

			// Both listed, the pictures' at the first picture.
			let listed = until(|| {
				let listed = catalog.snapshot().ext.studio.sources.remove("bed");
				listed.filter(|listed| !listed.video.renditions.is_empty() && !listed.audio.renditions.is_empty())
			})
			.await;
			let audio = listed.audio.renditions.keys().next().unwrap().clone();
			let video = listed.video.renditions.keys().next().unwrap().clone();
			let written = |id: &str| publish.stats().get(id).map_or(0, |rendition| rendition.frames);

			let feed = sources.get("bed").unwrap();
			pictures(feed, 0, 4).await;
			until(|| (written(&audio) > 0).then_some(())).await;
			assert_eq!(written(&video), 0, "heard, not shown");
			assert_eq!(sessions.open(), 0, "no encoder for the pictures");

			// Shown too, its pictures encode as they play, past what a
			// pipelined encoder holds back.
			publish.set_on_program(OnProgram {
				shown: ["bed".to_string()].into(),
				heard: ["bed".to_string()].into(),
			});
			pictures(feed, 4, 6).await;
			until(|| (written(&video) > 0).then_some(())).await;
		})
		.await;
	}

	/// A looping file whose sound is shorter than its pictures keeps its sound
	/// track on time: each loop's sound starts where the loop does, the
	/// silence between kept, rather than right after the last loop's sound.
	#[tokio::test(start_paused = true)]
	async fn a_looping_media_file_keeps_its_sound_on_time() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			std::fs::write(dir.path().join("bed.mkv"), crate::media::tests::UNEVEN).unwrap();
			let (publish, catalog, consumer) = studio();
			publish.set_on_program(heard("bed"));
			let (mut context, _origin) = context(dir.path());
			context.publish = Some(publish.clone());
			let mut collection = Collection::new();
			let bed = Source::Media {
				path: "bed.mkv".into(),
				looping: true,
				restart: false,
			};
			collection.sources.insert("bed".into(), bed);
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;

			let listed = until(|| {
				let listed = catalog.snapshot().ext.studio.sources.remove("bed");
				listed.filter(|listed| !listed.audio.renditions.is_empty())
			})
			.await;
			let (name, config) = listed.audio.renditions.iter().next().unwrap();
			let track = consumer
				.track(hang::catalog::Reference::track(config, name))
				.unwrap()
				.subscribe(
					moq_net::track::Subscription::default().with_delay(moq_net::track::Delay::default().with_max(None)),
				)
				.await
				.unwrap();
			let container = moq_mux::catalog::hang::Container::try_from(config).unwrap();
			let mut track = moq_mux::container::Consumer::new(track, container);
			// Two and a half loops of a second, each with half a second of sound.
			let begun = pictures(sources.get("bed").unwrap(), 0, 25).await;
			let now = kio::Waiter::noop();
			let mut stamps = Vec::new();
			let silences = until(|| {
				while let std::task::Poll::Ready(read) = track.poll_read(&now) {
					if let moq_mux::container::Read::Frame { frame, .. } = read.unwrap().expect("open") {
						stamps.push(std::time::Duration::from(frame.timestamp));
					}
				}
				let gaps = stamps.windows(2).map(|pair| pair[1].saturating_sub(pair[0]));
				let silences = gaps.filter(|gap| *gap > std::time::Duration::from_millis(300)).count();
				(silences >= 2).then_some(silences)
			})
			.await;
			assert_eq!(silences, 2, "each loop's silence is kept: {stamps:?}");
			// Stamped with when it sounded, though written ahead of it.
			let sounded = std::time::Duration::from(publish.catalog.clock().capture(begun).unwrap());
			assert_eq!(stamps[0], sounded, "the first sound as it sounded");
		})
		.await;
	}

	/// A file of sound alone lists its sound's track, live, and no pictures.
	#[tokio::test(start_paused = true)]
	async fn an_audio_file_lists_its_sound() {
		local(async {
			let dir = tempfile::tempdir().unwrap();
			std::fs::write(dir.path().join("tone.webm"), crate::media::tests::TONE).unwrap();
			let (publish, catalog, _consumer) = studio();
			let (mut context, _origin) = context(dir.path());
			context.publish = Some(publish);
			let mut collection = Collection::new();
			let tone = Source::Media {
				path: "tone.webm".into(),
				looping: false,
				restart: false,
			};
			collection.sources.insert("tone".into(), tone);
			let mut sources = Sources::new(context);
			let drivers = sources.sync(&collection);
			start(&sources, drivers).await;
			let listed = until(|| {
				let listed = catalog.snapshot().ext.studio.sources.remove("tone");
				listed.filter(|listed| !listed.audio.renditions.is_empty())
			})
			.await;
			assert!(listed.video.renditions.is_empty());
			assert_eq!(sources.get("tone").unwrap().track(), Some(Status::Live));
		})
		.await;
	}
}
