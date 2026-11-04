//! Each local source's own track in the studio's broadcast.
//!
//! A source publishes what it shows beside the program, so a viewer can
//! compose the scene itself. Its renditions are listed in a hang catalog of its
//! own, nested by source name under the studio catalog's [`SECTION`] section.
//! The root `video` and `audio` sections stay the program's alone: hang reads
//! their entries as alternatives of one picture, so a camera listed there would
//! be played, or packaged for HLS, as a variant of the program.
//!
//! A source encodes while it runs, whatever the demand, so a late viewer finds
//! its recent past; MoQ uploads only what a subscriber pulls.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::Error;

/// The studio catalog's own root section, namespaced as hang asks of
/// application sections.
pub const SECTION: &str = "dev.moq.studio";

/// The application sections of the studio catalog.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ext {
	/// The [`SECTION`] section, left off the wire while nothing is published.
	#[serde(rename = "dev.moq.studio", default, skip_serializing_if = "Section::is_empty")]
	pub studio: Section,
}

impl moq_mux::catalog::hang::CatalogExt for Ext {}

/// What the studio lists beside the program.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
	/// Each publishing source's catalog, by source name. Its tracks are in the
	/// studio's broadcast, and its timestamps are on the studio catalog's clock.
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub sources: BTreeMap<String, hang::Catalog>,
}

impl Section {
	fn is_empty(&self) -> bool {
		self.sources.is_empty()
	}
}

/// The studio catalog.
pub type Catalog = moq_mux::catalog::Producer<Ext>;

/// Where sources publish: the studio's broadcast and its catalog.
#[derive(Clone)]
pub struct Publish {
	broadcast: moq_net::broadcast::Producer,
	catalog: Catalog,
	bandwidth: moq_net::bandwidth::Allocator,
	kind: moq_video::encode::Kind,
	owners: Arc<Mutex<Owners>>,
}

/// Which [`Claim`] owns each source name's entry in the studio catalog, and
/// each open [`Listing`]'s catalog by its claim.
#[derive(Default)]
struct Owners {
	next: u64,
	by_name: BTreeMap<String, u64>,
	catalogs: BTreeMap<u64, moq_mux::catalog::Producer>,
}

impl Publish {
	/// Publish sources into `broadcast`, listed in `catalog`, the one serving it,
	/// encoding captured video with `kind`. Each source track claims its bitrate
	/// on `bandwidth`, the allocator the program's tracks claim on.
	///
	/// Pass [`Kind::Hardware`](moq_video::encode::Kind::Hardware) on a machine
	/// with a hardware encoder, so a session it refuses fails that source's track
	/// with the reason rather than quietly loading the CPU with a software one.
	pub fn new(
		broadcast: moq_net::broadcast::Producer,
		catalog: Catalog,
		bandwidth: moq_net::bandwidth::Allocator,
		kind: moq_video::encode::Kind,
	) -> Self {
		Self {
			broadcast,
			catalog,
			bandwidth,
			kind,
			owners: Default::default(),
		}
	}

	/// What each source track the studio catalog lists has written, keyed by
	/// rendition ID, as [`Catalog::stats`](moq_mux::catalog::Producer::stats)
	/// reports the program's. The broadcast names every track uniquely, so all
	/// sources share one map. A replaced source's tracks drop out with its
	/// catalog entry, even while it is still stopping.
	pub fn stats(&self) -> BTreeMap<String, hang::stats::Rendition> {
		let owners = self.owners.lock().unwrap();
		let listed = owners.by_name.values().filter_map(|id| owners.catalogs.get(id));
		listed.flat_map(|catalog| catalog.stats()).collect()
	}

	/// Claim `name`'s entry in the studio catalog for a source that is opening,
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

/// A source's hold on its name's entry in the studio catalog, until a newer
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

/// One source's catalog, listed in the studio's for as long as this lives and
/// its claim holds.
pub(crate) struct Listing<'a> {
	claim: &'a Claim,
	kind: moq_video::encode::Kind,
	broadcast: moq_net::broadcast::Producer,
	studio: Catalog,
	catalog: moq_mux::catalog::Producer,
	/// Where the nested catalog writes its own track, which nobody reads: its
	/// content reaches viewers through the studio catalog instead.
	_scratch: moq_net::broadcast::Producer,
}

impl<'a> Listing<'a> {
	pub(crate) fn new(claim: &'a Claim, publish: &Publish) -> Result<Self, Error> {
		let mut scratch = moq_net::broadcast::Info::new().produce();
		// The studio's clock, so source and program timestamps share one timeline.
		let config = moq_mux::catalog::Config::default()
			.with_clock(publish.catalog.clock())
			.with_bandwidth(publish.bandwidth.clone())
			.with_hls(None);
		let catalog = moq_mux::catalog::Producer::new(&mut scratch, config)?;
		claim.owners.lock().unwrap().catalogs.insert(claim.id, catalog.clone());
		Ok(Self {
			claim,
			kind: publish.kind.clone(),
			broadcast: publish.broadcast.clone(),
			studio: publish.catalog.clone(),
			catalog,
			_scratch: scratch,
		})
	}

	/// Copy every update of this source's catalog into the studio's, until
	/// either catalog closes.
	pub(crate) async fn mirror(&self) -> Result<(), Error> {
		let mut updates = self.catalog.consume()?;
		let mut studio = self.studio.clone();
		while let Some(mut catalog) = updates.next().await? {
			// Already at the studio catalog's root.
			catalog.clock = None;
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
		// Fails only once the studio catalog closed, which took the entry with it.
		let _ = self.studio.mutate(|root| {
			if self.claim.owned() {
				root.ext.studio.sources.remove(&self.claim.name);
			}
		});
	}
}

/// A captured source's video, encoded frame by frame as it is shown.
pub(crate) struct Video {
	/// Taken only to abort or finish it, as the track ends.
	producer: Option<moq_video::encode::Producer>,
	config: moq_video::encode::Config,
	/// Opened on the first frame, and reopened when the picture's size or color
	/// space changes, which moves the rendition to a new track.
	encoder: Option<moq_video::encode::Encoder>,
	clock: moq_mux::Clock,
	/// Where the capture's private timeline starts on the studio clock, in
	/// microseconds, taken at the first frame.
	epoch: Option<u128>,
}

impl Video {
	/// Advertise a rendition for pictures of `size`. Opens a throwaway encoder
	/// to learn exactly what the real one will write, so a machine that cannot
	/// encode another stream, such as a GPU at its hardware session cap, fails
	/// here with the reason.
	#[cfg_attr(
		not(feature = "capture"),
		allow(dead_code, reason = "only capture sources encode video")
	)]
	pub(crate) async fn new(
		listing: &Listing<'_>,
		size: moq_video::Size,
		framerate: moq_video::FrameRate,
		color: Option<moq_video::Color>,
	) -> Result<Self, Error> {
		let size = encodable(size);
		let mut config = moq_video::encode::Config::new(size.width, size.height, framerate);
		config.color = color;
		config.kind = listing.kind.clone();
		let rendition = config.probe().await?;
		let reserved = listing
			.catalog
			.reserve()
			.with_hls_hint(config.gop.duration(config.framerate));
		let producer = moq_video::encode::Producer::new(listing.broadcast.clone(), reserved, rendition)?;
		Ok(Self {
			producer: Some(producer),
			config,
			encoder: None,
			clock: listing.studio.clock(),
			epoch: None,
		})
	}

	/// Encode and publish `frame`, restamping it onto the studio clock.
	#[cfg_attr(
		not(feature = "capture"),
		allow(dead_code, reason = "only capture sources encode video")
	)]
	pub(crate) fn encode(&mut self, frame: &mut moq_video::Frame) -> Result<(), Error> {
		let captured = frame.timestamp.as_micros();
		let epoch = *self
			.epoch
			.get_or_insert_with(|| self.clock.now().as_micros().saturating_sub(captured));
		frame.timestamp = moq_net::Timestamp::from_micros((epoch + captured) as u64).map_err(moq_video::Error::from)?;

		let size = encodable(frame.size());
		let color = frame.surface.color();
		if size != self.config.size() || color.is_some_and(|color| Some(color) != self.config.color) {
			self.config.width = size.width;
			self.config.height = size.height;
			self.config.color = color;
			self.drain()?;
		}
		let encoder = match &mut self.encoder {
			Some(encoder) => encoder,
			None => self.encoder.insert(moq_video::encode::Encoder::new(&self.config)?),
		};
		let encoded = if frame.size() == size {
			encoder.encode(frame)?
		} else {
			encoder.encode(&frame.resize(size, &Default::default())?)?
		};
		self.producer.as_mut().expect("open until it ends").publish(&encoded)?;
		Ok(())
	}

	/// Publish what the encoder still holds and close it, so a pipelined
	/// backend's last pictures are not lost when the encoder is replaced or the
	/// track ends.
	fn drain(&mut self) -> Result<(), Error> {
		if let (Some(encoder), Some(producer)) = (self.encoder.take(), &mut self.producer) {
			producer.publish(&encoder.finish()?)?;
		}
		Ok(())
	}

	/// End the track, failed with `err` so viewers see why.
	#[cfg_attr(
		not(feature = "capture"),
		allow(dead_code, reason = "only capture sources encode video")
	)]
	pub(crate) fn abort(mut self, err: &Error) {
		if let Some(producer) = self.producer.take() {
			producer.abort(moq_net::Error::Transport(err.to_string()));
		}
	}
}

impl Drop for Video {
	fn drop(&mut self) {
		// A capture that ends, or a source removed or edited, ends its track
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

/// The size an encoder takes for a picture of `size`: even, since the encoder
/// subsamples chroma. An odd picture is scaled down to it.
fn encodable(size: moq_video::Size) -> moq_video::Size {
	moq_video::Size::new((size.width & !1).max(2), (size.height & !1).max(2))
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
	use crate::source::test::{context, local, start};
	use crate::source::{Sources, Status};

	/// A studio broadcast and catalog to publish into, and a reader of it.
	fn studio() -> (Publish, Catalog, moq_net::broadcast::Consumer) {
		let mut broadcast = moq_net::broadcast::Info::new().produce();
		let consumer = broadcast.consume();
		let config = moq_mux::catalog::Config::default().with_catalog(Default::default());
		let catalog = Catalog::new(&mut broadcast, config).unwrap();
		let bandwidth = moq_net::bandwidth::Allocator::unlimited();
		let publish = Publish::new(broadcast, catalog.clone(), bandwidth, moq_video::encode::Kind::Auto);
		(publish, catalog, consumer)
	}

	/// Yield until `f` holds of the studio catalog's source listings.
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
			assert!(published["red"].clock.is_none(), "the root carries the clock");
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

	/// A mid-gray picture of `size` at `millis` on a capture's own timeline.
	fn gray(size: moq_video::Size, millis: u64) -> moq_video::Frame {
		let i420 = moq_video::I420::new(size, vec![0x80; moq_video::I420::len(size).unwrap()]).unwrap();
		let timestamp = moq_net::Timestamp::from_micros(millis * 1000).unwrap();
		moq_video::Frame::new(moq_video::Surface::I420(i420), timestamp)
	}

	/// A captured picture encodes into its source's own listing, and one that
	/// changes size reopens the encoder rather than failing the track.
	#[tokio::test]
	async fn a_video_follows_its_pictures_size() {
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

			tokio::select! {
				result = listing.mirror() => panic!("the mirror ended: {result:?}"),
				() = async {
					let small = moq_video::Size::new(64, 48);
					let mut video = Video::new(&listing, small, framerate, None).await.unwrap();
					for millis in [0, 33, 66] {
						video.encode(&mut gray(small, millis)).unwrap();
					}
					let snapshot = listed(&catalog, |sources| width(sources) == Some(64)).await;
					assert!(snapshot.video.renditions.is_empty(), "no source is a program rendition");

					let large = moq_video::Size::new(80, 60);
					for millis in [100, 133] {
						video.encode(&mut gray(large, millis)).unwrap();
					}
					let snapshot = listed(&catalog, |sources| width(sources) == Some(80)).await;

					// Removing the source ends its track cleanly.
					let name = snapshot.ext.studio.sources["camera"].video.renditions.keys().next().unwrap();
					let mut track = consumer.track(name).unwrap().subscribe(None).await.unwrap();
					drop(video);
					while let Some(mut group) = track.recv_group().await.unwrap() {
						while group.read_frame().await.unwrap().is_some() {}
					}
				} => {}
			}

			// Every frame written counts for the Health panel, while the
			// studio catalog lists the source.
			let stats = publish.stats();
			assert_eq!(
				stats.values().map(|rendition| rendition.frames).sum::<u64>(),
				5,
				"{stats:?}"
			);
			// A replacement takes the listing, and the old tracks with it.
			let _newer = publish.claim("camera");
			assert!(publish.stats().is_empty());
			drop(listing);
			assert!(publish.owners.lock().unwrap().catalogs.is_empty());
		})
		.await;
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
}
