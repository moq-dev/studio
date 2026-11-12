//! The composition: a broadcast of the studio's own whose catalog lists other
//! broadcasts, the program and each source published independently, local or
//! a guest's, with where and when each shows and how its sound mixes, so a
//! player places each as the studio would have.
//!
//! The program leaves a published source out, so a player draws each
//! [picture](Composition::pictures) over the program, back to front, and mixes
//! each [sound](Child::sound) with the program's audio. A composition carries
//! only what every player can reproduce: a picture's place, crop, opacity,
//! visibility, and keyframes, and a sound's gain, mute, pan, and delay.
//! [`Collection::validate`] refuses a published source placed with anything
//! more, such as a blur or a blend mode.
//!
//! Each listed broadcast keeps its own timeline. The composition's media clock
//! is the program's, and each [`Child`] carries the offset that puts its
//! broadcast's timestamps on it: zero for a local source, which the studio
//! stamps on the program's clock, and for a guest, how far its stamps sat from
//! the program's when the studio played them.
//!
//! It travels as JSON on [`TRACK`] in snapshot mode, in a broadcast of its own
//! [beneath the program's](PATH), so a viewer joining late reads the
//! composition in force. The format is the studio's own, provisional until
//! hang specifies a composition.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_with::{DisplayFromStr, DurationMicroSeconds, serde_as};

use crate::Error;
use crate::collection::{Audio, Collection, Item, Layer, Size};

/// The composition broadcast's track carrying the composition, listed in its
/// catalog's `json` section.
pub const TRACK: &str = "live.kixel.studio.composition";

/// Where the composition's broadcast is, beneath the program's.
pub const PATH: &str = "composition";

/// Where each local source published independently has a broadcast of its
/// own, beneath the program's, under its name: `<program>/source/<name>`.
pub const SOURCES: &str = "source";

/// How far a child's offset may wander before a new composition carries it.
///
/// The offset is measured from a playout clock that re-anchors on every write
/// of a guest's sound, so it jitters by a millisecond or so while the guest
/// plays steadily. Well under the 45 ms of sound leading the picture a viewer
/// can detect (ITU-R BT.1359), it keeps a viewer's guest in step with the
/// studio's without a group per frame.
pub const DRIFT: Duration = Duration::from_millis(10);

/// Where and when each broadcast the composition lists shows or plays.
#[serde_as]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Composition {
	/// The format's version, first so a reader of another one fails on it.
	pub version: Version,
	/// The program's canvas, which pictures are placed on in pixels.
	pub canvas: Size,
	/// When the scene on program went on, on the composition's media clock:
	/// the moment its pictures' keyframes play from. In microseconds on the
	/// wire.
	#[serde_as(as = "DurationMicroSeconds<u64>")]
	pub start: Duration,
	/// The program, whose clock the composition's is.
	pub program: Program,
	/// Each source published independently that a picture shows or the
	/// program's audio plays, by source name.
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub sources: BTreeMap<String, Child>,
	/// Each picture over the program, back to front: the scene on program's,
	/// then the top layer's. Each is a scene file item whose `source` names a
	/// key of [`sources`](Self::sources), whose filters are only opacity, and
	/// whose keyframes play from [`start`](Self::start).
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub pictures: Vec<Item>,
}

impl Composition {
	/// The composition of `collection` with the scene called `scene` on
	/// program since `start` on the media clock, or no scene for `None`,
	/// drawn over `program`. Each source published independently is listed
	/// as `children` locates it, its sound filled in; one `children` leaves
	/// out, such as a guest offline, is left out with its pictures, as the
	/// studio shows nothing of it either.
	pub fn new(
		collection: &Collection,
		scene: Option<&str>,
		start: Duration,
		program: Program,
		mut children: BTreeMap<String, Child>,
	) -> Self {
		let pictures: Vec<Item> = pictures(collection, scene, true)
			.into_iter()
			.filter(|item| children.contains_key(&item.source))
			.collect();
		let heard = collection.program_audio().map(|audio| collection.heard(audio));
		for source in heard.into_iter().flatten() {
			if let Some(child) = children.get_mut(source) {
				child.sound = Some(collection.audio.get(source).cloned().unwrap_or_default());
			}
		}
		children.retain(|name, child| child.sound.is_some() || pictures.iter().any(|item| item.source == *name));
		Self {
			version: Version,
			canvas: Size {
				width: collection.canvas.width,
				height: collection.canvas.height,
			},
			start,
			program,
			sources: children,
			pictures,
		}
	}
}

/// Each item of the scene called `scene`, then of the top layer if `top`,
/// showing a source published independently, back to front, shown or hidden:
/// its `source` names the published source, and its crop and filters fold in
/// its source's, as the compositor draws them.
pub(crate) fn pictures(collection: &Collection, scene: Option<&str>, top: bool) -> Vec<Item> {
	let scene = scene.filter(|scene| collection.scene(scene).is_some());
	let places = scene.map(Layer::scene).into_iter().chain(top.then(Layer::top));
	let mut pictures = Vec::new();
	for place in places {
		for item in collection.items(&place).unwrap_or_default() {
			let source = collection.capture(&item.source);
			if !collection.published.contains(source) {
				continue;
			}
			pictures.push(Item {
				source: source.to_string(),
				crop: collection.crop(item),
				filters: collection.filters(item).cloned().collect(),
				..item.clone()
			});
		}
	}
	pictures
}

/// The format's version: deserializing refuses any but
/// [`Version::CURRENT`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "u32", try_from = "u32")]
pub struct Version;

impl Version {
	/// The version this build reads and writes. Version 1 was the layer
	/// layout, which named tracks of the program's broadcast.
	pub const CURRENT: u32 = 2;
}

impl From<Version> for u32 {
	fn from(_: Version) -> Self {
		Version::CURRENT
	}
}

impl TryFrom<u32> for Version {
	type Error = String;

	fn try_from(found: u32) -> Result<Self, Self::Error> {
		match found {
			Self::CURRENT => Ok(Self),
			found => Err(format!(
				"composition version {found} is not supported; this build reads version {}",
				Self::CURRENT
			)),
		}
	}
}

/// The program's broadcast.
#[serde_as]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Program {
	/// Its path, relative to the composition's, as hang's references are.
	pub broadcast: moq_net::path::Relative,
	/// The stream whose timeline the composition's clock is, so a player
	/// waits for it rather than placing anything on another's.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	#[serde_as(as = "Option<DisplayFromStr>")]
	pub instance: Option<moq_net::Instance>,
}

/// One broadcast the composition places over the program or mixes with it.
#[serde_as]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Child {
	/// Its path, relative to the composition's, as hang's references are.
	pub broadcast: moq_net::path::Relative,
	/// The stream [`offset`](Self::offset) holds for, or `None` for whichever
	/// serves the path, as for a source the studio stamps on the program's
	/// clock. A relay-minted instance is never named, since it cannot match
	/// another relay's.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	#[serde_as(as = "Option<DisplayFromStr>")]
	pub instance: Option<moq_net::Instance>,
	/// Microseconds added to its timestamps to put them on the composition's
	/// clock, negative for a broadcast stamped ahead of it.
	#[serde(default, skip_serializing_if = "is_zero")]
	pub offset: i64,
	/// Its gain, mute, pan, and delay in the program's audio, or `None` when
	/// the program's audio does not play it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub sound: Option<Audio>,
	/// The subscription priority a viewer fetches its tracks at, as moq-lite
	/// orders them.
	#[serde(default, skip_serializing_if = "is_zero")]
	pub priority: i8,
	/// Whether a viewer may switch it off.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub toggle: bool,
}

impl Child {
	/// A broadcast at `broadcast` relative to the composition's, of the
	/// stream `instance` if named, `offset` microseconds from the
	/// composition's clock, not yet heard.
	pub fn new(broadcast: moq_net::path::Relative, instance: Option<moq_net::Instance>, offset: i64) -> Self {
		Self {
			broadcast,
			instance: instance.filter(|instance| !instance.is_minted()),
			offset,
			sound: None,
			priority: 0,
			toggle: false,
		}
	}
}

fn is_zero<T: Default + PartialEq>(value: &T) -> bool {
	*value == T::default()
}

/// Publishes the composition on [`TRACK`] in a broadcast of its own, listed
/// in its catalog on the program's clock.
pub struct Publisher {
	catalog: moq_mux::catalog::Producer,
	broadcast: moq_net::broadcast::Producer,
	/// The track, and the composition last written to it, once there is one.
	track: Option<(moq_mux::json::Snapshot<Composition>, Option<Composition>)>,
}

impl Publisher {
	/// Publish in `broadcast`, whose catalog advertises `clock`, the
	/// program's, from the first [`update`](Self::update).
	pub fn new(broadcast: &mut moq_net::broadcast::Producer, clock: moq_mux::Clock) -> Result<Self, Error> {
		let config = moq_mux::catalog::Config::default().with_clock(clock);
		let catalog = moq_mux::catalog::Producer::new(broadcast, config)?;
		Ok(Self {
			catalog,
			broadcast: broadcast.clone(),
			track: None,
		})
	}

	/// Publish `composition`, stamped `at` on the media clock, when it took
	/// effect, unless it is the one published already, starting the track and
	/// its listing if there is none. A child's offset within [`DRIFT`] of the
	/// one published for the same stream keeps the published one.
	pub fn update(&mut self, mut composition: Composition, at: moq_net::Timestamp) -> Result<(), Error> {
		let (snapshot, published) = match &mut self.track {
			Some(track) => track,
			None => {
				let info = moq_net::track::Info::default()
					.with_timescale(moq_mux::Clock::TIMESCALE)
					.with_priority(hang::catalog::PRIORITY.catalog)
					.with_compress(true);
				let created = self.broadcast.create_track(TRACK, info)?;
				// Each composition whole in a group of its own, so a joiner needs
				// no refresh to read it.
				let options = moq_mux::json::Options::default().with_deltas(false);
				let config = moq_mux::json::Config::default();
				let snapshot = self.catalog.json_snapshot(created, config, options)?;
				self.track.insert((snapshot, None))
			}
		};
		if let Some(published) = published.as_ref() {
			steady(&mut composition, published);
			if *published == composition {
				return Ok(());
			}
		}
		snapshot.update(&composition, at)?;
		*published = Some(composition);
		Ok(())
	}

	/// End the track and the catalog, so a reader reaches their end.
	pub fn finish(&mut self) -> Result<(), Error> {
		if let Some((snapshot, _)) = self.track.take() {
			snapshot.finish()?;
		}
		Ok(self.catalog.finish()?)
	}
}

/// Keep each child's offset `published` carries for the same stream while the
/// new one is within [`DRIFT`] of it.
fn steady(composition: &mut Composition, published: &Composition) {
	let drift = DRIFT.as_micros() as i64;
	for (name, child) in &mut composition.sources {
		let Some(before) = published.sources.get(name) else { continue };
		if before.broadcast == child.broadcast
			&& before.instance == child.instance
			&& before.offset.abs_diff(child.offset) <= drift.unsigned_abs()
		{
			child.offset = before.offset;
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::collection::{Blend, Filter, Keyframe, Rect, Rgba, Scene, Source};
	use moq_mux::catalog::Stream;

	fn rect(x: f32) -> Rect {
		Rect {
			x,
			y: 0.0,
			width: 4.0,
			height: 4.0,
		}
	}

	fn relative(path: &str) -> moq_net::path::Relative {
		moq_net::path::Relative::new(path).unwrap()
	}

	/// A camera, a microphone, and a guest published, the camera placed over
	/// a color in scene `live`, behind a clone of itself in the top layer, the
	/// guest in scene `live`, and the microphone and the guest in the
	/// program's audio scene.
	fn collection() -> Collection {
		let mut collection = Collection::new();
		collection.sources.insert(
			"bg".into(),
			Source::Color {
				color: Rgba([0, 0, 0, 255]),
			},
		);
		collection.sources.insert("cam".into(), Source::Camera { id: None });
		collection
			.sources
			.insert("pip".into(), Source::Clone { source: "cam".into() });
		collection.sources.insert("mic".into(), Source::Microphone { id: None });
		collection.sources.insert(
			"alice".into(),
			Source::Broadcast {
				path: "guests/alice/camera.hang".into(),
			},
		);
		collection
			.published
			.extend(["cam".into(), "mic".into(), "alice".into()]);
		let mut cam = Item::new("cam", rect(0.0));
		cam.filters = vec![Filter::Opacity { amount: 0.5 }];
		cam.keyframes = vec![Keyframe {
			x: 10.0,
			..Keyframe::new(Duration::from_secs(1))
		}];
		collection.scenes = vec![Scene {
			items: vec![
				Item::new("bg", Rect::fill(&collection.canvas)),
				cam,
				Item::new("alice", rect(20.0)),
			],
			..Scene::new("live")
		}];
		collection.top = vec![Item::new("pip", rect(8.0))];
		collection.audio_scenes[0].sounds.extend(["mic".into(), "alice".into()]);
		collection.audio.insert(
			"mic".into(),
			Audio {
				gain: 0.5,
				delay: Duration::from_millis(80),
				..Default::default()
			},
		);
		collection.validate().unwrap();
		collection
	}

	fn program() -> Program {
		Program {
			broadcast: relative("."),
			instance: None,
		}
	}

	/// The studio's broadcasts as they are located: each local source at
	/// offset zero, and the guest at its own offset.
	fn children() -> BTreeMap<String, Child> {
		BTreeMap::from([
			("cam".into(), Child::new(relative("source/cam"), None, 0)),
			("mic".into(), Child::new(relative("source/mic"), None, 0)),
			(
				"alice".into(),
				Child::new(relative("../guests/alice/camera.hang"), Some("abc".parse().unwrap()), -1_500),
			),
		])
	}

	/// The composition places each published picture of the scene on
	/// program, then the top layer's, naming the source a clone shows, and
	/// plays each published sound the program's audio holds; the rest stay in
	/// the program.
	#[test]
	fn composes_the_published_sources_on_program() {
		let collection = collection();
		let start = Duration::from_secs(12);
		let composition = Composition::new(&collection, Some("live"), start, program(), children());
		assert_eq!(composition.start, start);
		assert_eq!(composition.canvas.width, collection.canvas.width);
		let items = &composition.pictures;
		assert_eq!(items.len(), 3, "{items:?}");
		assert_eq!(items[0], collection.scenes[0].items[1]);
		assert_eq!(items[1].source, "alice");
		assert_eq!((items[2].source.as_str(), items[2].rect), ("cam", rect(8.0)));
		let sounds: Vec<(&str, Option<&Audio>)> = composition
			.sources
			.iter()
			.map(|(name, child)| (name.as_str(), child.sound.as_ref()))
			.collect();
		assert_eq!(
			sounds,
			[
				("alice", Some(&Audio::default())),
				("cam", None),
				("mic", Some(&collection.audio["mic"]))
			]
		);
		assert_eq!(composition.sources["alice"].offset, -1_500);

		let top = Composition::new(&collection, None, start, program(), children());
		assert_eq!(top.pictures.len(), 1, "only the top layer's");
	}

	/// A source left unlocated, such as a guest offline, is left out with its
	/// pictures and sound, and a relay-minted instance is never named.
	#[test]
	fn leaves_out_what_is_not_located() {
		let collection = collection();
		let mut children = children();
		children.remove("alice");
		let minted = Child::new(relative("source/mic"), Some("~minted".parse().unwrap()), 0);
		assert_eq!(minted.instance, None);
		let composition = Composition::new(&collection, Some("live"), Duration::ZERO, program(), children);
		assert!(!composition.sources.contains_key("alice"));
		assert!(composition.pictures.iter().all(|item| item.source != "alice"));
	}

	/// Each source's own crop and filters fold into its picture, a clone's
	/// its own rather than the source's it shows.
	#[test]
	fn folds_in_the_sources_crop_and_filters() {
		let mut collection = collection();
		let video = |amount| crate::collection::Video {
			crop: crate::collection::Crop {
				left: 0.25,
				..Default::default()
			},
			filters: vec![Filter::Opacity { amount }],
			..Default::default()
		};
		collection.video.insert("cam".into(), video(0.8));
		collection.video.insert("pip".into(), video(0.4));
		collection.validate().unwrap();
		let composition = Composition::new(&collection, Some("live"), Duration::ZERO, program(), children());
		let cams = composition.pictures.iter().filter(|item| item.source == "cam");
		let filters: Vec<&[Filter]> = cams.clone().map(|item| item.filters.as_slice()).collect();
		assert_eq!(
			filters,
			[
				&[Filter::Opacity { amount: 0.8 }, Filter::Opacity { amount: 0.5 }][..],
				&[Filter::Opacity { amount: 0.4 }][..],
			]
		);
		assert!(cams.into_iter().all(|item| item.crop.left == 0.25));
	}

	/// A published source placed with what a player cannot reproduce is
	/// refused, as is one in a scene another scene draws, which the
	/// composition cannot place.
	#[test]
	fn refuses_what_a_picture_cannot_carry() {
		let refused = |edit: &dyn Fn(&mut Collection)| {
			let mut collection = collection();
			edit(&mut collection);
			assert!(matches!(collection.validate(), Err(Error::Invalid(_))));
		};
		refused(&|collection| collection.scenes[0].items[1].blend = Blend::Screen);
		refused(&|collection| collection.scenes[0].items[1].filters = vec![Filter::Blur { radius: 2.0 }]);
		refused(&|collection| collection.scenes[0].items[2].blend = Blend::Screen);
		refused(&|collection| {
			collection.scenes[0].items[1].keyframes[0].filters = vec![Filter::Grayscale { amount: 1.0 }];
		});
		refused(&|collection| {
			collection.video.insert(
				"pip".into(),
				crate::collection::Video {
					filters: vec![Filter::Invert { amount: 1.0 }],
					..Default::default()
				},
			);
		});
		refused(&|collection| {
			collection
				.sources
				.insert("nest".into(), Source::Scene { scene: "live".into() });
			collection.scenes.push(Scene {
				items: vec![Item::new("nest", rect(0.0))],
				..Scene::new("outer")
			});
		});

		// Unpublished, the same camera takes them.
		let mut collection = collection();
		collection.published.remove("cam");
		collection.scenes[0].items[1].blend = Blend::Screen;
		collection.validate().unwrap();
	}

	/// A composition reads back as written, and one of another version is
	/// refused, naming it.
	#[test]
	fn refuses_another_version() {
		let composition = Composition::new(
			&collection(),
			Some("live"),
			Duration::from_micros(1_500),
			program(),
			children(),
		);
		let text = toml::to_string(&composition).unwrap();
		assert!(text.starts_with("version = 2\n"), "{text}");
		assert!(text.contains("start = 1500\n"), "{text}");
		let alice = "[sources.alice]\nbroadcast = \"../guests/alice/camera.hang\"\ninstance = \"abc\"\noffset = -1500\n";
		assert!(text.contains(alice), "{text}");
		assert_eq!(toml::from_str::<Composition>(&text).unwrap(), composition);

		let other = text.replacen("version = 2", "version = 1", 1);
		let err = toml::from_str::<Composition>(&other).unwrap_err().to_string();
		assert!(err.contains("composition version 1 is not supported"), "{err}");
	}

	/// The track carries each new composition at the media time it took
	/// effect, listed in the catalog on the program's clock; an offset that
	/// only jitters writes nothing, one that moves past the drift or a new
	/// instance does, and finishing ends the track and the catalog.
	#[tokio::test]
	async fn publishes_each_new_composition() {
		let mut broadcast = moq_net::broadcast::Info::new().produce();
		let clock = moq_mux::Clock::default();
		let mut publisher = Publisher::new(&mut broadcast, clock.clone()).unwrap();
		let collection = collection();
		let compose = |offset: i64, instance: &str| {
			let mut children = children();
			children.insert(
				"alice".into(),
				Child::new(relative("../guests/alice/camera.hang"), Some(instance.parse().unwrap()), offset),
			);
			Composition::new(&collection, Some("live"), Duration::ZERO, program(), children)
		};
		let stamp = |secs| moq_net::Timestamp::try_from(Duration::from_secs(secs)).unwrap();
		publisher.update(compose(0, "a"), stamp(1)).unwrap();

		let mut catalogs = moq_mux::catalog::Consumer::<()>::new(&broadcast.consume(), Default::default())
			.await
			.unwrap();
		let catalog = catalogs.next().await.unwrap().expect("a catalog");
		assert!(catalog.json.tracks.contains_key(TRACK));
		assert_eq!(catalog.media.clock, clock.wall(), "the program's clock");

		let track = broadcast.consume().track(TRACK).unwrap();
		let mut reader = moq_json::snapshot::Consumer::<Composition>::new(track, Default::default());
		let next = async |reader: &mut moq_json::snapshot::Consumer<Composition>| {
			let read = reader.next().await.unwrap().expect("a composition");
			(read.value.sources["alice"].clone(), read.at.map(Duration::from))
		};
		let (alice, at) = next(&mut reader).await;
		assert_eq!((alice.offset, at), (0, Some(Duration::from_secs(1))));

		let drift = DRIFT.as_micros() as i64;
		publisher.update(compose(drift, "a"), stamp(2)).unwrap();
		publisher.update(compose(-drift, "a"), stamp(3)).unwrap();
		publisher.update(compose(drift + 1, "a"), stamp(4)).unwrap();
		let (alice, at) = next(&mut reader).await;
		assert_eq!((alice.offset, at), (drift + 1, Some(Duration::from_secs(4))));
		publisher.update(compose(drift + 2, "b"), stamp(5)).unwrap();
		let (alice, at) = next(&mut reader).await;
		assert_eq!(alice.instance, Some("b".parse().unwrap()));
		assert_eq!((alice.offset, at), (drift + 2, Some(Duration::from_secs(5))));

		publisher.finish().unwrap();
		assert!(reader.next().await.unwrap().is_none(), "the track ends");
		// The track leaves the catalog, then the catalog ends.
		while catalogs.next().await.unwrap().is_some() {}
	}
}
