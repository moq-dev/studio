//! Items and filters copied as text, to paste into another scene or another
//! collection.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::collection::{
	Audio, Collection, Filter, Item, Layer, Mixes, Scene, Source, Video, validate_filters, versioned,
};
use crate::edit::Draft;

/// What a copy holds: items, or a filter chain.
///
/// Written with [`to_toml`](Self::to_toml) in the scene file's format, so a
/// clip is as readable as the file, and parsed with
/// [`from_toml`](Self::from_toml), which validates it as strictly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "clip", rename_all = "snake_case")]
pub enum Clip {
	/// Items and what they show.
	Items(Items),
	/// Filters, to add to a source's or an item's.
	Filters(Filters),
}

/// Copied items, with the sources they show and those sources' settings, so
/// they paste into a collection that lacks them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Items {
	/// Back to front.
	pub items: Vec<Item>,
	/// Every source an item shows or a clone among them clones.
	pub sources: BTreeMap<String, Source>,
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub audio: BTreeMap<String, Audio>,
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub video: BTreeMap<String, Video>,
}

/// A copied filter chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filters {
	/// In order.
	pub filters: Vec<Filter>,
}

impl Clip {
	/// A copy of `items` of `collection`, with what they show and what their
	/// filters read. Relative image and LUT paths are resolved against `dir`,
	/// the absolute directory of the collection's scene file, so they name the
	/// same file wherever the clip is pasted.
	pub fn items(collection: &Collection, items: Vec<Item>, dir: &Path) -> Self {
		let mut names: Vec<String> = items.iter().map(|item| item.source.clone()).collect();
		names.extend(items.iter().flat_map(|item| &item.filters).filter_map(read));

		let mut clip = Items {
			items,
			sources: BTreeMap::new(),
			audio: BTreeMap::new(),
			video: BTreeMap::new(),
		};
		// Each source brings what it clones and what its own filters read.
		while let Some(name) = names.pop() {
			let Some(source) = collection.sources.get(&name) else {
				continue;
			};
			if clip.sources.contains_key(&name) {
				continue;
			}
			let mut source = source.clone();
			if let Some(path) = source.path_mut() {
				*path = dir.join(&path);
			}
			clip.sources.insert(name.clone(), source);
			names.push(collection.capture(&name).to_string());
			if let Some(audio) = collection.audio.get(&name) {
				clip.audio.insert(name.clone(), audio.clone());
			}
			if let Some(video) = collection.video.get(&name) {
				names.extend(video.filters.iter().filter_map(read));
				clip.video.insert(name, video.clone());
			}
		}
		Self::Items(clip)
	}

	/// Serialize, `version` first.
	pub fn to_toml(&self) -> Result<String, Error> {
		let body = toml::to_string(self).map_err(|err| Error::Format(err.to_string()))?;
		Ok(format!("version = {}\n{body}", Collection::VERSION))
	}

	/// Parse and validate a clip, refusing any other text.
	pub fn from_toml(text: &str) -> Result<Self, Error> {
		let table = versioned(text)?;
		let clip = Self::deserialize(table).map_err(|err| Error::Format(err.to_string()))?;
		match &clip {
			// The rules a collection holding just the clip would break, such as
			// an item showing a source the clip lacks. It has whatever mixes the
			// clip's sources are left out of, which pasting checks.
			Self::Items(items) => {
				let excluded = items.audio.values().flat_map(|audio| &audio.exclude);
				let mut names: BTreeSet<String> = excluded.cloned().collect();
				names.insert(Mixes::DEFAULT.into());
				Collection {
					sources: items.sources.clone(),
					mixes: Mixes {
						names: names.into_iter().collect(),
						program: Mixes::DEFAULT.into(),
					},
					audio: items.audio.clone(),
					video: items.video.clone(),
					// In a scene, where keyframes play; pasting into the top
					// layer checks it holds none.
					scenes: vec![Scene {
						items: items.items.clone(),
						..Scene::new("clip")
					}],
					..Collection::new()
				}
				.validate()?
			}
			Self::Filters(filters) => validate_filters(&filters.filters, "the copied filters")?,
		}
		Ok(clip)
	}
}

/// The source `filter` reads, if any.
fn read(filter: &Filter) -> Option<String> {
	filter.source().map(str::to_string)
}

impl Collection {
	/// `base`, or `base 2`, `base 3`, and so on: the first no source is named.
	pub fn unused_name(&self, base: &str) -> String {
		std::iter::once(base.to_string())
			.chain((2..).map(|n| format!("{base} {n}")))
			.find(|name| !self.sources.contains_key(name))
			.expect("an unused name")
	}
}

impl Draft {
	/// Add `items` in front of everything in `layer`, of the collection whose
	/// scene file is in the absolute directory `dir`.
	///
	/// A source the collection has under the same name and definition is the
	/// one shown, so items pasted within a collection are linked to the ones
	/// copied. A source whose name is taken by a different one is added under
	/// a new name, which clones and filters naming it follow, and an added
	/// source brings its audio and video settings, left out of only the mixes
	/// both collections have. An image or LUT under `dir`
	/// is named relative to it, as the scene file would.
	pub fn paste(&mut self, layer: &Layer, items: &Items, dir: &Path) -> Result<(), Error> {
		let mut names = BTreeMap::new();
		let mut added = Vec::new();
		// What clones clone first, so each clone names what it was renamed to.
		let (clones, origins): (Vec<_>, Vec<_>) = items
			.sources
			.iter()
			.partition(|(_, source)| matches!(source, Source::Clone { .. }));
		for (name, source) in origins.into_iter().chain(clones) {
			let mut source = match source {
				Source::Clone { source } => Source::Clone {
					source: names.get(source).cloned().unwrap_or_else(|| source.clone()),
				},
				source => source.clone(),
			};
			// A file is the same source if it names the same file, however
			// either spells the path.
			let resolve = |source: &Source| {
				let mut source = source.clone();
				if let Some(path) = source.path_mut() {
					*path = dir.join(&path);
				}
				source
			};
			let existing = self.collection().sources.get(name).map(resolve);
			if existing.as_ref() == Some(&resolve(&source)) {
				names.insert(name.clone(), name.clone());
				continue;
			}
			if let Some(path) = source.path_mut() {
				*path = path.strip_prefix(dir).unwrap_or(path).to_path_buf();
			}
			let renamed = self.collection().unused_name(name);
			self.set_source(&renamed, Some(source))?;
			names.insert(name.clone(), renamed.clone());
			added.push((name, renamed));
		}

		// Filters name sources by their pasted names, once every one is known.
		let rename = |filters: &[Filter]| -> Vec<Filter> {
			let mut filters = filters.to_vec();
			for name in filters.iter_mut().filter_map(Filter::source_mut) {
				if let Some(renamed) = names.get(name) {
					*name = renamed.clone();
				}
			}
			filters
		};
		for (name, renamed) in added {
			if let Some(audio) = items.audio.get(name) {
				// Left out only of the mixes this collection has too.
				let mut audio = audio.clone();
				let mixes = &self.collection().mixes.names;
				audio.exclude.retain(|mix| mixes.contains(mix));
				self.set_audio(&renamed, Some(audio))?;
			}
			if let Some(video) = items.video.get(name) {
				let filters = rename(&video.filters);
				self.set_video(
					&renamed,
					Some(Video {
						filters,
						..video.clone()
					}),
				)?;
			}
		}
		for item in &items.items {
			let source = names.get(&item.source).cloned().unwrap_or_else(|| item.source.clone());
			let index = self
				.collection()
				.items(layer)
				.ok_or_else(|| Error::Conflict(format!("there is no {layer}")))?
				.len();
			let filters = rename(&item.filters);
			self.insert_item(
				layer,
				index,
				Item {
					source,
					filters,
					..item.clone()
				},
			)?;
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::collection::{MaskMode, Rect, Rgba};

	fn color(rgba: [u8; 4]) -> Source {
		Source::Color { color: Rgba(rgba) }
	}

	/// A camera, a faded clone of it, and a scene showing the clone.
	fn collection() -> Collection {
		let mut collection = Collection::new();
		collection.sources.insert("cam".into(), Source::Camera { id: None });
		collection
			.sources
			.insert("cam faded".into(), Source::Clone { source: "cam".into() });
		collection.video.insert(
			"cam faded".into(),
			Video {
				filters: vec![Filter::Opacity { amount: 0.5 }],
				..Default::default()
			},
		);
		collection.scenes[0]
			.items
			.push(Item::new("cam faded", Rect::fill(&collection.canvas)));
		collection.validate().unwrap();
		collection
	}

	const DIR: &str = "/shows/one";

	fn paste(collection: &Collection, layer: &Layer, text: &str, dir: &str) -> Collection {
		let Clip::Items(items) = Clip::from_toml(text).unwrap() else {
			panic!("not items")
		};
		let mut draft = collection.draft("Paste");
		draft.paste(layer, &items, Path::new(dir)).unwrap();
		let mut pasted = collection.clone();
		pasted.apply(&draft.finish().unwrap()).unwrap();
		pasted
	}

	/// Within a collection, a pasted item shows the very source copied.
	#[test]
	fn pastes_linked_within_a_collection() {
		let collection = collection();
		let clip = Clip::items(&collection, collection.scenes[0].items.clone(), Path::new(DIR));
		let pasted = paste(&collection, &Layer::Top, &clip.to_toml().unwrap(), DIR);
		assert_eq!(pasted.sources, collection.sources, "no source added");
		assert_eq!(pasted.top, collection.scenes[0].items);
	}

	/// An animated item copies with its keyframes into a scene, and the top
	/// layer, which never goes on to play them, refuses it.
	#[test]
	fn pastes_keyframes_into_a_scene_only() {
		let mut collection = collection();
		collection.scenes[0].items[0].keyframes = vec![crate::Keyframe {
			x: -100.0,
			..crate::Keyframe::new(std::time::Duration::ZERO)
		}];
		let clip = Clip::items(&collection, collection.scenes[0].items.clone(), Path::new(DIR));
		let text = clip.to_toml().unwrap();
		let pasted = paste(&collection, &Layer::Scene("Scene".into()), &text, DIR);
		assert_eq!(pasted.scenes[0].items[1], collection.scenes[0].items[0]);

		let Clip::Items(items) = Clip::from_toml(&text).unwrap() else {
			panic!("not items")
		};
		let mut draft = collection.draft("Paste");
		draft.paste(&Layer::Top, &items, Path::new(DIR)).unwrap();
		assert!(matches!(draft.finish(), Err(Error::Invalid(_))));
	}

	/// Into another collection, a clip brings what its items show, renaming a
	/// source whose name is taken by another, and its clone follows the rename.
	#[test]
	fn pastes_into_another_collection() {
		let source = collection();
		let text = Clip::items(&source, source.scenes[0].items.clone(), Path::new(DIR))
			.to_toml()
			.unwrap();

		let mut other = Collection::new();
		other.sources.insert("cam".into(), color([0, 255, 0, 255]));
		let pasted = paste(&other, &Layer::Scene("Scene".into()), &text, DIR);
		assert_eq!(pasted.sources["cam"], color([0, 255, 0, 255]), "left alone");
		assert_eq!(pasted.sources["cam 2"], Source::Camera { id: None });
		assert_eq!(pasted.sources["cam faded"], Source::Clone { source: "cam 2".into() });
		assert_eq!(pasted.video["cam faded"], source.video["cam faded"]);
		assert_eq!(pasted.scenes[0].items, source.scenes[0].items);
	}

	/// A pasted source keeps its sound's settings, left out of only the mixes
	/// the collection it lands in has.
	#[test]
	fn pastes_audio_into_the_mixes_there() {
		let mut source = Collection::new();
		source.mixes.names.push("Stream".into());
		source.sources.insert(
			"guest".into(),
			Source::Broadcast {
				path: "room/guest".into(),
			},
		);
		let audio = crate::Audio {
			gain: 0.5,
			exclude: ["Program".to_string(), "Stream".to_string()].into(),
			..Default::default()
		};
		source.audio.insert("guest".into(), audio.clone());
		source.scenes[0]
			.items
			.push(Item::new("guest", Rect::fill(&source.canvas)));
		source.validate().unwrap();
		let text = Clip::items(&source, source.scenes[0].items.clone(), Path::new(DIR))
			.to_toml()
			.unwrap();

		let pasted = paste(&Collection::new(), &Layer::Top, &text, DIR);
		let expected = crate::Audio {
			exclude: ["Program".to_string()].into(),
			..audio
		};
		assert_eq!(pasted.audio["guest"], expected);
	}

	/// A relative image path names the same file wherever the clip is pasted,
	/// and stays relative, and linked, within its own collection.
	#[test]
	fn an_image_names_the_same_file_anywhere() {
		let mut collection = Collection::new();
		collection.sources.insert(
			"logo".into(),
			Source::Image {
				path: "logo.png".into(),
			},
		);
		collection.scenes[0]
			.items
			.push(Item::new("logo", Rect::fill(&collection.canvas)));
		let text = Clip::items(&collection, collection.scenes[0].items.clone(), Path::new(DIR))
			.to_toml()
			.unwrap();

		let home = paste(&collection, &Layer::Top, &text, DIR);
		assert_eq!(home.sources, collection.sources, "linked to the same image");

		// However the scene file spells the path.
		let mut absolute = collection.clone();
		absolute.sources.insert(
			"logo".into(),
			Source::Image {
				path: "/shows/one/logo.png".into(),
			},
		);
		let home = paste(&absolute, &Layer::Top, &text, DIR);
		assert_eq!(home.sources, absolute.sources, "linked to the same image");

		let mut other = Collection::new();
		other.sources.insert(
			"logo".into(),
			Source::Image {
				path: "logo.png".into(),
			},
		);
		let away = paste(&other, &Layer::Top, &text, "/shows/two");
		assert_eq!(away.sources["logo"], other.sources["logo"], "another file, left alone");
		assert_eq!(
			away.sources["logo 2"],
			Source::Image {
				path: "/shows/one/logo.png".into()
			}
		);
	}

	/// Copied items bring the sources their filters read, and a renamed one is
	/// renamed in the filters too.
	#[test]
	fn filters_bring_what_they_read() {
		let mut collection = collection();
		collection.sources.insert(
			"matte".into(),
			Source::Image {
				path: "matte.png".into(),
			},
		);
		collection.sources.insert(
			"grade".into(),
			Source::Lut {
				path: "grade.cube".into(),
			},
		);
		let mask = |source: &str| Filter::Mask {
			source: source.into(),
			mode: MaskMode::Luminance,
		};
		collection.scenes[0].items[0].filters = vec![mask("matte")];
		collection
			.video
			.get_mut("cam faded")
			.unwrap()
			.filters
			.push(Filter::Lut {
				source: "grade".into(),
				amount: 0.5,
			});
		collection.validate().unwrap();
		let text = Clip::items(&collection, collection.scenes[0].items.clone(), Path::new(DIR))
			.to_toml()
			.unwrap();

		let mut other = Collection::new();
		other.sources.insert("matte".into(), color([0, 255, 0, 255]));
		let pasted = paste(&other, &Layer::Top, &text, "/shows/two");
		assert_eq!(pasted.top[0].filters, [mask("matte 2")]);
		assert_eq!(
			pasted.sources["grade"],
			Source::Lut {
				path: "/shows/one/grade.cube".into()
			}
		);
		assert_eq!(pasted.video["cam faded"], collection.video["cam faded"]);
	}

	#[test]
	fn filters_round_trip() {
		let clip = Clip::Filters(Filters {
			filters: vec![Filter::Opacity { amount: 0.25 }],
		});
		assert_eq!(Clip::from_toml(&clip.to_toml().unwrap()).unwrap(), clip);
	}

	/// Clipboard text comes from anywhere, so a clip is held to the scene
	/// file's rules.
	#[test]
	fn refuses_what_a_scene_file_would() {
		let collection = collection();
		let text = Clip::items(&collection, collection.scenes[0].items.clone(), Path::new(DIR))
			.to_toml()
			.unwrap();
		let dangling = text.replace("[sources.cam]", "[sources.other]");
		assert!(
			matches!(Clip::from_toml(&dangling), Err(Error::Invalid(_))),
			"{dangling}"
		);
		let faint = text.replace("amount = 0.5", "amount = 2.0");
		assert!(matches!(Clip::from_toml(&faint), Err(Error::Invalid(_))), "{faint}");
		for text in [
			"hello",
			"version = 1\nclip = \"scenes\"",
			"version = 2\nclip = \"filters\"\nfilters = []",
		] {
			assert!(Clip::from_toml(text).is_err(), "{text}");
		}
	}
}
