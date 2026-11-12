//! Return feeds: what each guest in the room sees and hears of the show.
//!
//! Every guest the studio renews gets a broadcast at
//! `.return/<identity>/<generation>` in the room, which only their token may
//! read. Its video is the program's: the studio's session in the room serves the
//! program broadcast once at `.program`, and each feed's catalog lists its video
//! renditions by reference, so a guest costs no video encode. Its audio is its
//! own: a mix-minus of the audio scene on program without the guest's microphone, so
//! they follow the show without hearing themselves back.
//!
//! The studio's session in the room publishes them, holding only what the room
//! key signs. A guest costs one subtraction per audio block and an Opus encoder
//! for as long as they are renewed, so the cost is bounded by the guests the
//! studio signed for and kept in the room.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use moq_mux::catalog::Stream;
use moq_net::Path;
use moq_scene::Collection;
use moq_scene::source::Sources;

use crate::guests::{PROGRAM, Room};
use crate::mix;

/// A guest's return audio: speech and the show's sound, for following along,
/// not for broadcast.
const BITRATE: moq_net::bandwidth::Rate = moq_net::bandwidth::Rate::from_kbps(64);

/// The return feeds of every renewed guest, and the program they show.
pub struct Feeds {
	room: Arc<Room>,
	/// What the studio's session in the room publishes.
	origin: moq_net::origin::Producer,
	/// The program broadcast, which each feed's video names.
	program: moq_net::broadcast::Consumer,
	/// The program catalog's clock, which each feed's catalog shares so its
	/// audio lines up with the program's video.
	clock: moq_mux::Clock,
	/// Where the program audio's timestamps start on that clock.
	epoch: Duration,
	feeds: BTreeMap<String, Feed>,
	/// Serves the program at [`PROGRAM`] until dropped.
	serving: tokio::task::JoinHandle<()>,
	/// Where each feed's task runs: the runtime the feeds were made on, never
	/// the program thread [`sync`](Self::sync) is called from, so encoding a
	/// guest's audio never holds up a frame.
	runtime: tokio::runtime::Handle,
}

impl Feeds {
	/// Serve `program` at [`PROGRAM`] in `room`, through `origin`, the studio's
	/// session there, and publish a feed for each guest [`sync`](Self::sync)
	/// finds renewed. `clock` and `epoch` are the program audio's, which the
	/// feeds' audio is stamped against.
	pub fn new(
		room: Arc<Room>,
		origin: moq_net::origin::Producer,
		program: moq_net::broadcast::Consumer,
		clock: moq_mux::Clock,
		epoch: Duration,
	) -> anyhow::Result<Self> {
		let route = origin
			.route_prefix(PROGRAM, Default::default())
			.context("cannot serve the program in the room")?;
		let served = program.clone();
		let serving = tokio::spawn(async move {
			while let Some(request) = route.requested().await {
				// The route claims everything beneath it too, which holds nothing.
				match request.path().as_str() == PROGRAM {
					true => request.serve(served.clone()),
					false => request.reject(moq_net::Error::NotFound),
				}
			}
		});
		Ok(Self {
			room,
			origin,
			program,
			clock,
			epoch,
			feeds: BTreeMap::new(),
			serving,
			runtime: tokio::runtime::Handle::current(),
		})
	}

	/// Publish a feed for each renewed guest and end the others', and point
	/// each at the audio scene `collection` puts on program, without the
	/// guest's microphone as `sources` runs it.
	pub fn sync(&mut self, collection: &Collection, sources: &Sources) {
		let renewed = self.room.renewed();
		// A feed whose task stopped, having logged why, opens again below.
		self.feeds.retain(|identity, feed| {
			renewed.contains(&(identity.clone(), feed.generation)) && !feed.task.is_finished()
		});
		let on_program = collection.program_audio().unwrap_or_default();
		let program = sources.mix(on_program);
		for (identity, generation) in renewed {
			if !self.feeds.contains_key(&identity) {
				match self.open(&identity, generation) {
					Ok(feed) => self.feeds.insert(identity.clone(), feed),
					Err(err) => {
						tracing::error!(%identity, error = format!("{err:#}"), "cannot publish a guest's return feed");
						continue;
					}
				};
			}
			let Some(program) = program else { continue };
			let pick = &self.feeds[&identity].pick;
			// A mix-minus takes out one strip, so the copy of their camera the
			// program plays audibly, if any. A guest the host never added, or
			// removed, is in no mix to take out.
			let voices: Vec<&str> = self.room.voices(collection, &identity).collect();
			let heard = voices.iter().find(|name| {
				let strip = sources.get(name).and_then(|source| source.strip());
				let audible = strip.is_some_and(|strip| strip.gain() > 0.0);
				audible && sources.plays(on_program, name)
			});
			let voice = heard.or(voices.first());
			match voice.and_then(|name| sources.get(name)?.strip()) {
				Some(strip) => pick.set(program.minus(strip)),
				None => pick.set(program.clone()),
			}
		}
	}

	/// Announce a feed for `identity` in `generation`, its audio following the
	/// mix-minus it is picked.
	fn open(&self, identity: &str, generation: u64) -> anyhow::Result<Feed> {
		let path = Room::feed_path(identity, generation)?;
		// Rendition references resolve against the feed's own path.
		let program = Path::new(PROGRAM)?
			.relative(&path)
			.context("the program cannot be named from a feed")?;
		let publication = self.origin.route(path, moq_net::origin::RouteOptions::hidden())?;
		let mut broadcast = publication.broadcast().clone();
		let config = moq_mux::catalog::Config::default().with_clock(self.clock.clone());
		let catalog = moq_mux::catalog::Producer::new(&mut broadcast, config)?;

		let bus = moq_audio::bus::Config::default();
		let mut input = moq_audio::encode::Input::new(bus.sample_rate, bus.layout);
		input.format = moq_audio::Format::F32;
		let mut options = moq_audio::encode::Options::default();
		options.track = Some("audio".into());
		options.config = moq_audio::encode::Config::from_input(moq_audio::encode::Codec::Opus, &input);
		options.config.bitrate = Some(BITRATE);
		let audio = moq_audio::encode::Producer::new(&mut broadcast, catalog.clone(), input, &options)?;
		publication.announce()?;

		let pick = mix::Pick::default();
		let (stop, stopped) = tokio::sync::oneshot::channel();
		let running = Running {
			_publication: publication,
			catalog,
			audio,
			follow: pick.follow(),
			program: self.program.clone(),
			reference: program,
			epoch: self.epoch,
			rate: bus.sample_rate,
		};
		let identity = identity.to_string();
		let task = self.runtime.spawn(async move {
			if let Err(err) = running.run(stopped).await {
				tracing::error!(%identity, error = format!("{err:#}"), "a guest's return feed stopped");
			}
		});
		Ok(Feed {
			generation,
			pick,
			_stop: stop,
			task,
		})
	}
}

impl Drop for Feeds {
	fn drop(&mut self) {
		self.serving.abort();
	}
}

/// One guest's return feed, which ends when dropped.
struct Feed {
	/// The generation it is published under.
	generation: u64,
	/// The mix-minus its audio reads.
	pick: mix::Pick,
	/// Dropped to end the feed.
	_stop: tokio::sync::oneshot::Sender<()>,
	/// Runs the feed until it ends.
	task: tokio::task::JoinHandle<()>,
}

/// A feed's broadcast, as its task runs it.
struct Running {
	/// Ends the broadcast once the task returns.
	_publication: moq_net::origin::Route,
	catalog: moq_mux::catalog::Producer,
	audio: moq_audio::encode::Producer,
	follow: mix::Follow,
	/// The program broadcast, whose video renditions the catalog lists.
	program: moq_net::broadcast::Consumer,
	/// The program, relative to the feed.
	reference: moq_net::path::Relative,
	epoch: Duration,
	rate: u32,
}

impl Running {
	/// List the program's video and encode the picked audio until `stopped`
	/// or the program ends, then finish the audio track and the catalog.
	async fn run(mut self, stopped: tokio::sync::oneshot::Receiver<()>) -> anyhow::Result<()> {
		let Self {
			catalog,
			audio,
			follow,
			program,
			reference,
			epoch,
			rate,
			..
		} = &mut self;
		let ended = tokio::select! {
			listed = list(program, catalog, reference) => listed,
			encoded = mix::encode(follow, audio, *epoch, *rate) => encoded,
			// Dropped with the feed.
			_ = stopped => Ok(()),
		};
		if let Err(err) = ended {
			self.audio.abort(moq_net::StreamError::Internal.into());
			self.catalog.abort(moq_net::StreamError::Internal.into());
			return Err(err);
		}
		self.audio.finish()?;
		self.catalog.finish()?;
		Ok(())
	}
}

/// List the video renditions of each catalog `program` publishes in
/// `catalog`, read from the broadcast at `reference`, until the program ends.
async fn list(
	program: &moq_net::broadcast::Consumer,
	catalog: &mut moq_mux::catalog::Producer,
	reference: &moq_net::path::Relative,
) -> anyhow::Result<()> {
	let format = moq_mux::catalog::CatalogFormat::Hang;
	let mut listings = moq_mux::catalog::Consumer::<()>::new(program, format)
		.await
		.context("cannot read the program's catalog")?;
	while let Some(listed) = listings.next().await? {
		let mut video = listed.video;
		for rendition in video.renditions.values_mut() {
			rendition.broadcast = Some(reference.clone());
		}
		catalog.mutate(|catalog| catalog.video = video)?;
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use moq_audio::bus::{Bus, Live};
	use moq_scene::source::{Mixed, Mixer};
	use moq_scene::{History, Source};
	use url::Url;

	use super::*;
	use crate::guests::tests::{EXPIRY, Relay, key};

	/// The default bus's 10 ms block.
	const BLOCK: usize = 480;

	const TIMEOUT: Duration = Duration::from_secs(10);

	/// A program broadcast listing one video rendition, `video`.
	fn program() -> (moq_net::broadcast::Producer, moq_mux::catalog::Producer) {
		let mut broadcast = moq_net::broadcast::Info::new().produce();
		let mut catalog = moq_mux::catalog::Producer::new(&mut broadcast, Default::default()).unwrap();
		let codec: hang::catalog::VideoCodec = "avc1.64001f".parse().unwrap();
		let config = hang::catalog::VideoConfig::new(codec);
		catalog
			.mutate(|catalog| {
				catalog.video.renditions.insert("video".into(), config);
			})
			.unwrap();
		(broadcast, catalog)
	}

	/// Return feeds on `origin` for the guests in `room`, of a program that
	/// lives as long as the returned broadcast and catalog.
	fn feeds(
		room: &Arc<Room>,
		origin: &moq_net::origin::Producer,
	) -> (Feeds, (moq_net::broadcast::Producer, moq_mux::catalog::Producer)) {
		let (broadcast, catalog) = program();
		let feeds = Feeds::new(
			room.clone(),
			origin.clone(),
			broadcast.consume(),
			catalog.clock(),
			Duration::ZERO,
		);
		(feeds.unwrap(), (broadcast, catalog))
	}

	/// Alice and bob in `room` with their cameras on, and a collection playing
	/// each camera, whose sources run on `bus`, which `mixed` follows.
	fn guests(room: &Room, bus: &Bus, mixed: Mixed) -> (Collection, Sources) {
		let mut collection = Collection::new();
		for guest in ["alice", "bob"] {
			let camera = format!("{guest}/camera.hang");
			room.observe(&Path::new(&camera).unwrap(), true, &collection, &History::new());
			let path = format!("guests/{camera}");
			collection.sources.insert(guest.into(), Source::Broadcast { path });
			collection.audio_scenes[0].sounds.insert(guest.into());
		}
		let origin = moq_tokio::origin::spawn();
		let mut sources = Sources::new(moq_scene::source::Context {
			bus: bus.clone(),
			mixed,
			origin: origin.consume(),
			dir: ".".into(),
			output: moq_video::Output::Cpu,
			browser: None,
			publish: None,
		});
		// Not run: the test feeds each guest's strip itself.
		drop(sources.sync(&collection));
		(collection, sources)
	}

	/// A stereo channel into `bus` under the strip of the source `name`.
	fn channel(bus: &Bus, sources: &Sources, name: &str) -> Live {
		let strip = sources.get(name).unwrap().strip().unwrap().clone();
		let mut input = moq_audio::encode::Input::new(48_000, moq_audio::Layout::Stereo);
		input.format = moq_audio::Format::F32;
		bus.live(input, strip).unwrap()
	}

	fn feed(channel: &mut Live, value: f32) {
		let data: Vec<u8> = std::iter::repeat_n(value, BLOCK * 2)
			.flat_map(f32::to_le_bytes)
			.collect();
		channel.write(&data).unwrap();
	}

	/// The catalog `broadcast` lists once it lists video.
	async fn listed(broadcast: &moq_net::broadcast::Consumer) -> hang::Catalog {
		let format = moq_mux::catalog::CatalogFormat::Hang;
		let mut listings = moq_mux::catalog::Consumer::<()>::new(broadcast, format).await.unwrap();
		loop {
			let catalog = listings.next().await.unwrap().expect("a catalog");
			if !catalog.video.renditions.is_empty() {
				return catalog;
			}
		}
	}

	/// Each guest's feed lists the program's video by reference and audio of
	/// its own, which hears everyone on program but that guest, and ends
	/// cleanly once the guest is no longer renewed.
	#[tokio::test]
	async fn each_guest_hears_the_program_without_themselves() {
		let _clock = kio::clock::Manual::new().install(&tokio::runtime::Handle::current());
		let connect = Url::parse("https://relay.example/show").unwrap();
		let page = Url::parse("https://studio.example/join/").unwrap();
		let key = key("show/guests", &["**"], &["**"]);
		let room = Arc::new(Room::new(&connect, "guests", key, page, EXPIRY).unwrap());
		let origin = moq_tokio::origin::spawn();
		let (mut feeds, _program) = feeds(&room, &origin);
		let (bus, driver) = Bus::new(Default::default()).unwrap();
		let (mut driver, mixed) = Mixer::new(driver, kio::clock::now());
		let (mut collection, mut sources) = guests(&room, &bus, mixed);
		// Copies of alice's camera listed first, one muted and one with its
		// fader down, which nobody hears on program, so her mix-minus takes
		// out the copy they do.
		let quiet = [
			(
				"Alice muted",
				moq_scene::Audio {
					muted: true,
					..Default::default()
				},
			),
			(
				"Alice silent",
				moq_scene::Audio {
					gain: 0.0,
					..Default::default()
				},
			),
		];
		for (name, audio) in quiet {
			let copy = collection.sources["alice"].clone();
			collection.sources.insert(name.into(), copy);
			collection.audio.insert(name.into(), audio);
			collection.audio_scenes[0].sounds.insert(name.into());
		}
		drop(sources.sync(&collection));
		let mut alice = channel(&bus, &sources, "alice");
		let mut bob = channel(&bus, &sources, "bob");
		// From a thread of its own, as the program thread syncs: the feeds run
		// on the runtime they were made on, not the caller's.
		std::thread::scope(|scope| scope.spawn(|| feeds.sync(&collection, &sources)).join().unwrap());

		let path = Room::feed_path("alice", 0).unwrap();
		let broadcast = origin
			.consume()
			.follow(path, None)
			.unwrap()
			.available()
			.await
			.unwrap()
			.expect("an instance");
		let catalog = listed(&broadcast).await;
		let video = &catalog.video.renditions["video"];
		assert_eq!(video.broadcast.as_ref().unwrap().as_str(), "../../.program");
		assert!(catalog.audio.renditions.contains_key("audio"), "{catalog:?}");

		let mut heard_by_alice = feeds.feeds["alice"].pick.follow();
		let mut heard_by_bob = feeds.feeds["bob"].pick.follow();
		// Past the bus's latency, so each mix carries both guests.
		let mut last = (None, None);
		for _ in 0..20 {
			feed(&mut alice, 0.25);
			feed(&mut bob, 0.5);
			driver.advance(BLOCK);
			last = (heard_by_alice.read().await, heard_by_bob.read().await);
		}
		let (alice_hears, bob_hears) = (last.0.unwrap(), last.1.unwrap());
		assert_eq!(alice_hears.data[0], 0.5, "alice hears only bob");
		assert_eq!(bob_hears.data[0], 0.25, "bob hears only alice");

		let path = Room::feed_path("bob", 0).unwrap();
		let bob = origin
			.consume()
			.follow(path, None)
			.unwrap()
			.available()
			.await
			.unwrap()
			.expect("an instance");
		let format = moq_mux::catalog::CatalogFormat::Hang;
		let mut listings = moq_mux::catalog::Consumer::<()>::new(&bob, format).await.unwrap();
		room.stop_renewing("bob").unwrap();
		feeds.sync(&collection, &sources);
		assert_eq!(feeds.feeds.keys().collect::<Vec<_>>(), ["alice"]);
		// A guest who is not renewed has no feed, whose catalog ends cleanly.
		while listings.next().await.expect("the catalog finishes").is_some() {}
		bob.closed().await;
	}

	/// Through a relay that verifies tokens, a guest reads their own feed and
	/// the program it shows, but not another guest's feed.
	#[tokio::test(flavor = "multi_thread")]
	async fn a_guest_reads_only_their_own_feed_through_a_relay() {
		let relay = Relay::start().await;
		let room = relay.room();
		let origin = room.origin().unwrap().clone();
		let (mut feeds, _program) = feeds(&room, &origin);
		let (bus, driver) = Bus::new(Default::default()).unwrap();
		let (_driver, mixed) = Mixer::new(driver, kio::clock::now());
		let (collection, sources) = guests(&room, &bus, mixed);
		feeds.sync(&collection, &sources);

		let read = moq_tokio::origin::spawn();
		let link = room.invite("alice").unwrap().link;
		let _alice = relay.join(&link, &moq_tokio::origin::spawn(), &read);
		let follow = async |path: Path| {
			let broadcast = read.consume().follow(path, None)?.available().await?;
			moq_net::Result::Ok(broadcast.expect("the first change is an instance"))
		};

		let path = Room::feed_path("alice", 0).unwrap();
		let feed = kio::clock::timeout(TIMEOUT, follow(path.clone()))
			.await
			.expect("alice's feed reaches her")
			.unwrap();
		let catalog = kio::clock::timeout(TIMEOUT, listed(&feed)).await.unwrap();
		let reference = catalog.video.renditions["video"].broadcast.clone().unwrap();
		let program = path.resolve(&reference).unwrap();
		assert_eq!(program.as_str(), PROGRAM);
		let shown = kio::clock::timeout(TIMEOUT, follow(program))
			.await
			.expect("the program reaches alice")
			.unwrap();
		let shown = kio::clock::timeout(TIMEOUT, listed(&shown)).await.unwrap();
		assert!(shown.video.renditions.contains_key("video"));

		// Bob reads his own, so the relay routes it.
		let bob = moq_tokio::origin::spawn();
		let link = room.invite("bob").unwrap().link;
		let _bob = relay.join(&link, &moq_tokio::origin::spawn(), &bob);
		let path = Room::feed_path("bob", 0).unwrap();
		kio::clock::timeout(TIMEOUT, bob.consume().follow(path.clone(), None).unwrap().available())
			.await
			.expect("bob's feed reaches him")
			.unwrap();
		let other = kio::clock::timeout(TIMEOUT, read.consume().request(path, None))
			.await
			.expect("the relay answers");
		assert!(other.is_err(), "alice read bob's feed");
	}
}
