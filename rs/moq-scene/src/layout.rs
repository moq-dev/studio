//! The layer layout: where and when each source published independently
//! shows over the program, and how its sound plays, so a player places the
//! source's own track as the studio would have.
//!
//! The program leaves a published source out, so a player draws each
//! [`Picture`] over the program, back to front, and mixes each [`Sound`] with
//! the program's audio. A layout carries only what every player can
//! reproduce: a picture's place, crop, opacity, visibility, and keyframes, and
//! a sound's gain, mute, pan, and delay. [`Collection::validate`] refuses a
//! published source placed with anything more, such as a blur or a blend mode.
//!
//! It travels as JSON on the program broadcast's [`TRACK`] in snapshot mode,
//! so a viewer joining late reads the layout in force. The format is the
//! studio's own, provisional until hang specifies a layer layout.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_with::{DurationMicroSeconds, serde_as};

use crate::Error;
use crate::collection::{Audio, Collection, Item, Layer, Size};
use crate::publish::Catalog;

/// The program broadcast's track carrying the layout, listed in its catalog's
/// `json` section.
pub const TRACK: &str = "live.kixel.studio.layers";

/// Each source published independently that the program shows or plays, and
/// how.
#[serde_as]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
	/// The format's version, first so a reader of another one fails on it.
	pub version: Version,
	/// The program's canvas, which pictures are placed on in pixels.
	pub canvas: Size,
	/// When the scene on program went on, on the broadcast's media clock: the
	/// moment its pictures' keyframes play from. In microseconds on the wire.
	#[serde_as(as = "DurationMicroSeconds<u64>")]
	pub start: Duration,
	/// Each picture over the program, back to front: the scene on program's,
	/// then the top layer's.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub pictures: Vec<Picture>,
	/// Each sound the program's audio plays, by source name.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub sounds: Vec<Sound>,
}

impl Layout {
	/// The layout of `collection` with the scene called `scene` on program
	/// since `start` on the media clock, or no scene for `None`.
	pub fn new(collection: &Collection, scene: Option<&str>, start: Duration) -> Self {
		let pictures = pictures(collection, scene, true)
			.into_iter()
			.map(Picture::new)
			.collect();
		let heard = collection.program_audio().map(|audio| collection.heard(audio));
		let sounds = heard.into_iter().flatten();
		let sounds = sounds.filter(|source| collection.published.contains(*source));
		let sounds = sounds
			.map(|source| Sound {
				source: source.to_string(),
				audio: collection.audio.get(source).cloned().unwrap_or_default(),
				priority: 0,
				toggle: false,
			})
			.collect();
		Self {
			version: Version,
			canvas: Size {
				width: collection.canvas.width,
				height: collection.canvas.height,
			},
			start,
			pictures,
			sounds,
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

/// The layout format's version: deserializing refuses any but
/// [`Version::CURRENT`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "u32", try_from = "u32")]
pub struct Version;

impl Version {
	/// The version this build reads and writes.
	pub const CURRENT: u32 = 1;
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
				"layer layout version {found} is not supported; this build reads version {}",
				Self::CURRENT
			)),
		}
	}
}

/// A published source's picture over the program.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Picture {
	/// Where and how it shows: its `source` names the source's entry under the
	/// catalog's `live.kixel.studio` `sources`, its filters are only opacity, and
	/// its keyframes play from the layout's `start`.
	pub item: Item,
	/// The subscription priority a viewer fetches its track at, as moq-lite
	/// orders them.
	#[serde(default, skip_serializing_if = "is_zero")]
	pub priority: i8,
	/// Whether a viewer may switch it off.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub toggle: bool,
}

impl Picture {
	fn new(item: Item) -> Self {
		Self {
			item,
			priority: 0,
			toggle: false,
		}
	}
}

/// A published source's sound in the program's audio.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sound {
	/// The source's entry under the catalog's `live.kixel.studio` `sources`.
	pub source: String,
	/// Its gain, mute, pan, and delay, applied to its track's sound.
	pub audio: Audio,
	/// The subscription priority a viewer fetches its track at, as moq-lite
	/// orders them.
	#[serde(default, skip_serializing_if = "is_zero")]
	pub priority: i8,
	/// Whether a viewer may switch it off.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub toggle: bool,
}

fn is_zero(priority: &i8) -> bool {
	*priority == 0
}

/// Publishes the layout on [`TRACK`] in a broadcast, listed in its catalog,
/// while there is one.
pub struct Publisher {
	broadcast: moq_net::broadcast::Producer,
	catalog: Catalog,
	/// The track, and the layout last written to it, while there is one.
	track: Option<(moq_mux::json::Snapshot<Layout>, Option<Layout>)>,
}

impl Publisher {
	/// Publish in `broadcast`, listed in `catalog`, from the first
	/// [`update`](Self::update).
	pub fn new(broadcast: moq_net::broadcast::Producer, catalog: Catalog) -> Self {
		Self {
			broadcast,
			catalog,
			track: None,
		}
	}

	/// Publish `layout`, stamped `at` on the media clock, when it took
	/// effect, unless it is the one published already, starting the track and
	/// its listing if there is none.
	pub fn update(&mut self, layout: Layout, at: moq_net::Timestamp) -> Result<(), Error> {
		let (snapshot, published) = match &mut self.track {
			Some(track) => track,
			None => {
				let info = moq_net::track::Info::default()
					.with_timescale(moq_mux::Clock::TIMESCALE)
					.with_priority(hang::catalog::PRIORITY.catalog)
					.with_compress(true);
				let created = self.broadcast.create_track(TRACK, info)?;
				// Each layout whole in a group of its own, so a joiner needs no
				// refresh to read it.
				let options = moq_mux::json::Options::default().with_deltas(false);
				let config = moq_mux::json::Config::default();
				let snapshot = self.catalog.json_snapshot(created, config, options)?;
				self.track.insert((snapshot, None))
			}
		};
		if published.as_ref() == Some(&layout) {
			return Ok(());
		}
		snapshot.update(&layout, at)?;
		*published = Some(layout);
		Ok(())
	}

	/// End the track, so a reader reaches its end, and take it out of the
	/// catalog, until the next [`update`](Self::update).
	pub fn finish(&mut self) -> Result<(), Error> {
		match self.track.take() {
			Some((snapshot, _)) => Ok(snapshot.finish()?),
			None => Ok(()),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::collection::{Blend, Filter, Keyframe, Rect, Rgba, Scene, Source};

	fn rect(x: f32) -> Rect {
		Rect {
			x,
			y: 0.0,
			width: 4.0,
			height: 4.0,
		}
	}

	/// A camera and a microphone published, the camera placed over a color in
	/// scene `live`, behind a clone of itself in the top layer, and the
	/// microphone in the program's audio scene.
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
		collection.published.extend(["cam".into(), "mic".into()]);
		let mut cam = Item::new("cam", rect(0.0));
		cam.filters = vec![Filter::Opacity { amount: 0.5 }];
		cam.keyframes = vec![Keyframe {
			x: 10.0,
			..Keyframe::new(Duration::from_secs(1))
		}];
		collection.scenes = vec![Scene {
			items: vec![Item::new("bg", Rect::fill(&collection.canvas)), cam],
			..Scene::new("live")
		}];
		collection.top = vec![Item::new("pip", rect(8.0))];
		collection.audio_scenes[0].sounds.insert("mic".into());
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

	/// The layout places each published picture of the scene on program, then
	/// the top layer's, naming the source a clone shows, and plays each
	/// published sound the program's audio holds; the rest stay in the
	/// program.
	#[test]
	fn lays_out_the_published_sources_on_program() {
		let collection = collection();
		let start = Duration::from_secs(12);
		let layout = Layout::new(&collection, Some("live"), start);
		assert_eq!(layout.start, start);
		assert_eq!(layout.canvas.width, collection.canvas.width);
		let items: Vec<&Item> = layout.pictures.iter().map(|picture| &picture.item).collect();
		assert_eq!(items.len(), 2, "{items:?}");
		assert_eq!(items[0], &collection.scenes[0].items[1]);
		assert_eq!((items[1].source.as_str(), items[1].rect), ("cam", rect(8.0)));
		let sounds: Vec<(&str, &Audio)> = layout
			.sounds
			.iter()
			.map(|sound| (sound.source.as_str(), &sound.audio))
			.collect();
		assert_eq!(sounds, [("mic", &collection.audio["mic"])]);

		let top = Layout::new(&collection, None, start);
		assert_eq!(top.pictures.len(), 1, "only the top layer's");
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
		let layout = Layout::new(&collection, Some("live"), Duration::ZERO);
		let filters: Vec<&[Filter]> = layout
			.pictures
			.iter()
			.map(|picture| picture.item.filters.as_slice())
			.collect();
		assert_eq!(
			filters,
			[
				&[Filter::Opacity { amount: 0.8 }, Filter::Opacity { amount: 0.5 }][..],
				&[Filter::Opacity { amount: 0.4 }][..],
			]
		);
		assert!(layout.pictures.iter().all(|picture| picture.item.crop.left == 0.25));
	}

	/// A published source placed with what a player cannot reproduce is
	/// refused, as is one in a scene another scene draws, which the layout
	/// cannot place.
	#[test]
	fn refuses_what_a_layer_cannot_carry() {
		let refused = |edit: &dyn Fn(&mut Collection)| {
			let mut collection = collection();
			edit(&mut collection);
			assert!(matches!(collection.validate(), Err(Error::Invalid(_))));
		};
		refused(&|collection| collection.scenes[0].items[1].blend = Blend::Screen);
		refused(&|collection| collection.scenes[0].items[1].filters = vec![Filter::Blur { radius: 2.0 }]);
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

	/// A layout reads back as written, and one of another version is refused,
	/// naming it.
	#[test]
	fn refuses_another_version() {
		let layout = Layout::new(&collection(), Some("live"), Duration::from_micros(1_500));
		let text = toml::to_string(&layout).unwrap();
		assert!(text.starts_with("version = 1\n"), "{text}");
		assert!(text.contains("start = 1500\n"), "{text}");
		assert_eq!(toml::from_str::<Layout>(&text).unwrap(), layout);

		let other = text.replacen("version = 1", "version = 2", 1);
		let err = toml::from_str::<Layout>(&other).unwrap_err().to_string();
		assert!(err.contains("layer layout version 2 is not supported"), "{err}");
	}

	/// The layout track carries each new layout at the media time it took
	/// effect, listed in the catalog while there is one; finishing ends it
	/// for a reader and takes it out of the catalog.
	#[tokio::test]
	async fn publishes_each_new_layout() {
		let mut broadcast = moq_net::broadcast::Info::new().produce();
		let config = moq_mux::catalog::Config::default().with_catalog(Default::default());
		let catalog = Catalog::new(&mut broadcast, config).unwrap();
		let collection = collection();
		let first = Layout::new(&collection, None, Duration::ZERO);
		let second = Layout::new(&collection, Some("live"), Duration::from_secs(1));
		let stamp = |secs| moq_net::Timestamp::try_from(Duration::from_secs(secs)).unwrap();
		let mut publisher = Publisher::new(broadcast.clone(), catalog.clone());
		publisher.update(first.clone(), stamp(1)).unwrap();
		let listed = |catalog: &hang::Catalog<crate::publish::Ext>| catalog.json.tracks.contains_key(TRACK);
		assert!(listed(&catalog.snapshot()));

		let track = broadcast.consume().track(TRACK).unwrap();
		let mut reader = moq_json::snapshot::Consumer::<Layout>::new(track, Default::default());
		let read = reader.next().await.unwrap().expect("a layout");
		assert_eq!(
			(read.value, read.at.map(Duration::from)),
			(first, Some(Duration::from_secs(1)))
		);
		publisher.update(second.clone(), stamp(2)).unwrap();
		publisher.update(second.clone(), stamp(3)).unwrap();
		let read = reader.next().await.unwrap().expect("a layout");
		assert_eq!(
			(read.value, read.at.map(Duration::from)),
			(second, Some(Duration::from_secs(2)))
		);

		publisher.finish().unwrap();
		assert!(
			reader.next().await.unwrap().is_none(),
			"the unchanged layout wrote nothing"
		);
		assert!(!listed(&catalog.snapshot()));
	}
}
