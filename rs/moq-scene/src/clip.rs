//! Items and filters copied as text, to paste into another scene or another
//! collection.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::collection::{Audio, Collection, Filter, Item, Layer, Source, Video, validate_filters, versioned};
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
	/// A copy of `items` of `collection`, with what they show. Relative image
	/// paths are resolved against `dir`, the absolute directory of the
	/// collection's scene file, so they name the same file wherever the clip is
	/// pasted.
	pub fn items(collection: &Collection, items: Vec<Item>, dir: &Path) -> Self {
		let mut names: Vec<String> = items.iter().map(|item| item.source.clone()).collect();
		names.extend(items.iter().map(|item| collection.capture(&item.source).to_string()));

		let mut clip = Items {
			items,
			sources: BTreeMap::new(),
			audio: BTreeMap::new(),
			video: BTreeMap::new(),
		};
		for name in names {
			let Some(source) = collection.sources.get(&name) else {
				continue;
			};
			let source = match source {
				Source::Image { path } => Source::Image { path: dir.join(path) },
				source => source.clone(),
			};
			clip.sources.insert(name.clone(), source);
			if let Some(audio) = collection.audio.get(&name) {
				clip.audio.insert(name.clone(), *audio);
			}
			if let Some(video) = collection.video.get(&name) {
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
			// an item showing a source the clip lacks.
			Self::Items(items) => Collection {
				sources: items.sources.clone(),
				audio: items.audio.clone(),
				video: items.video.clone(),
				scenes: Vec::new(),
				top: items.items.clone(),
				..Collection::new()
			}
			.validate()?,
			Self::Filters(filters) => validate_filters(&filters.filters, "the copied filters")?,
		}
		Ok(clip)
	}
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
	/// a new name, and an added source brings its audio and video settings. An
	/// image under `dir` is named relative to it, as the scene file would.
	pub fn paste(&mut self, layer: &Layer, items: &Items, dir: &Path) -> Result<(), Error> {
		let mut names = BTreeMap::new();
		// What clones clone first, so each clone names what it was renamed to.
		let (clones, origins): (Vec<_>, Vec<_>) = items
			.sources
			.iter()
			.partition(|(_, source)| matches!(source, Source::Clone { .. }));
		for (name, source) in origins.into_iter().chain(clones) {
			let source = match source {
				Source::Clone { source } => Source::Clone {
					source: names.get(source).cloned().unwrap_or_else(|| source.clone()),
				},
				source => source.clone(),
			};
			// An image is the same source if it names the same file, however
			// either spells the path.
			let resolve = |source: &Source| match source {
				Source::Image { path } => Source::Image { path: dir.join(path) },
				source => source.clone(),
			};
			let existing = self.collection().sources.get(name).map(resolve);
			if existing.as_ref() == Some(&resolve(&source)) {
				names.insert(name.clone(), name.clone());
				continue;
			}
			let source = match source {
				Source::Image { path } => Source::Image {
					path: path.strip_prefix(dir).unwrap_or(&path).to_path_buf(),
				},
				source => source,
			};
			let renamed = self.collection().unused_name(name);
			self.set_source(&renamed, Some(source))?;
			if let Some(audio) = items.audio.get(name) {
				self.set_audio(&renamed, Some(*audio))?;
			}
			if let Some(video) = items.video.get(name) {
				self.set_video(&renamed, Some(video.clone()))?;
			}
			names.insert(name.clone(), renamed);
		}

		for item in &items.items {
			let source = names.get(&item.source).cloned().unwrap_or_else(|| item.source.clone());
			let index = self
				.collection()
				.items(layer)
				.ok_or_else(|| Error::Conflict(format!("there is no {layer}")))?
				.len();
			self.insert_item(layer, index, Item { source, ..item.clone() })?;
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::collection::{Rect, Rgba};

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
