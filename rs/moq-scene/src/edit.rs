//! Edits to a [`Collection`], and the history that undoes and redoes them.
//!
//! Every change to a collection is an [`Edit`]: a labelled list of
//! [`Change`]s, each naming what it replaces as well as what replaces it. That
//! makes an edit its own undo ([`Edit::inverse`]) and lets it refuse to apply
//! to a collection that no longer holds what it expects, which is how a
//! history saved beside an edited file finds the entries it can still replay.
//!
//! Build an edit with a [`Draft`], which reads what each change replaces from
//! the collection as the earlier changes left it.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::collection::{Audio, Canvas, Collection, Filter, Item, Layer, Mixes, Scene, Source, Transition, Video};

/// One part of a collection, before and after an edit.
///
/// `None` is absent: a change from `None` adds, and one to `None` removes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
	/// A [`Collection::sources`] entry.
	Source {
		name: String,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		before: Option<Source>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		after: Option<Source>,
	},
	/// A [`Collection::audio`] entry.
	Audio {
		name: String,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		before: Option<Audio>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		after: Option<Audio>,
	},
	/// A [`Collection::video`] entry.
	Video {
		name: String,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		before: Option<Video>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		after: Option<Video>,
	},
	/// The scene at `index` of [`Collection::scenes`]. Adding one inserts it
	/// there, shifting the later ones back, and replacing one with another
	/// name renames it.
	Scene {
		index: usize,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		before: Option<Scene>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		after: Option<Scene>,
	},
	/// The item at `index` of `layer`. Adding one inserts it there, in front of
	/// the ones before it.
	Item {
		layer: Layer,
		index: usize,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		before: Option<Item>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		after: Option<Item>,
	},
	/// [`Collection::transition`].
	Transition { before: Transition, after: Transition },
	/// [`Collection::canvas`].
	Canvas { before: Canvas, after: Canvas },
	/// [`Collection::mixes`].
	Mixes { before: Mixes, after: Mixes },
}

impl Change {
	/// The change that puts back what this one replaced.
	pub fn inverse(&self) -> Self {
		match self.clone() {
			Self::Source { name, before, after } => Self::Source {
				name,
				before: after,
				after: before,
			},
			Self::Audio { name, before, after } => Self::Audio {
				name,
				before: after,
				after: before,
			},
			Self::Video { name, before, after } => Self::Video {
				name,
				before: after,
				after: before,
			},
			Self::Scene { index, before, after } => Self::Scene {
				index,
				before: after,
				after: before,
			},
			Self::Item {
				layer,
				index,
				before,
				after,
			} => Self::Item {
				layer,
				index,
				before: after,
				after: before,
			},
			Self::Transition { before, after } => Self::Transition {
				before: after,
				after: before,
			},
			Self::Canvas { before, after } => Self::Canvas {
				before: after,
				after: before,
			},
			Self::Mixes { before, after } => Self::Mixes {
				before: after,
				after: before,
			},
		}
	}

	/// The name of the source, audio, or video entry the change makes.
	fn entry(&self) -> Option<&str> {
		match self {
			Self::Source { name, .. } | Self::Audio { name, .. } | Self::Video { name, .. } => Some(name),
			Self::Scene { .. }
			| Self::Item { .. }
			| Self::Transition { .. }
			| Self::Canvas { .. }
			| Self::Mixes { .. } => None,
		}
	}

	/// Whether the change leaves its part as it was.
	fn is_noop(&self) -> bool {
		match self {
			Self::Source { before, after, .. } => before == after,
			Self::Audio { before, after, .. } => before == after,
			Self::Video { before, after, .. } => before == after,
			Self::Scene { before, after, .. } => before == after,
			Self::Item { before, after, .. } => before == after,
			Self::Transition { before, after } => before == after,
			Self::Canvas { before, after } => before == after,
			Self::Mixes { before, after } => before == after,
		}
	}

	/// `self` then `newer`, as one change, if both change the same part.
	fn merge(&self, newer: &Self) -> Option<Self> {
		let merged = match (self.clone(), newer) {
			(
				Self::Source { name, before, after },
				Self::Source {
					name: n,
					before: b,
					after: a,
				},
			) if name == *n && after == *b => Self::Source {
				name,
				before,
				after: a.clone(),
			},
			(
				Self::Audio { name, before, after },
				Self::Audio {
					name: n,
					before: b,
					after: a,
				},
			) if name == *n && after == *b => Self::Audio {
				name,
				before,
				after: a.clone(),
			},
			(
				Self::Video { name, before, after },
				Self::Video {
					name: n,
					before: b,
					after: a,
				},
			) if name == *n && after == *b => Self::Video {
				name,
				before,
				after: a.clone(),
			},
			// Only a replacement in place, so an index means the same scene or
			// item on both sides.
			(
				Self::Scene {
					index,
					before: Some(before),
					after: Some(after),
				},
				Self::Scene {
					index: i,
					before: Some(b),
					after: Some(a),
				},
			) if index == *i && after == *b => Self::Scene {
				index,
				before: Some(before),
				after: Some(a.clone()),
			},
			(
				Self::Item {
					layer,
					index,
					before: Some(before),
					after: Some(after),
				},
				Self::Item {
					layer: l,
					index: i,
					before: Some(b),
					after: Some(a),
				},
			) if layer == *l && index == *i && after == *b => Self::Item {
				layer,
				index,
				before: Some(before),
				after: Some(a.clone()),
			},
			(Self::Transition { before, after }, Self::Transition { before: b, after: a }) if after == *b => {
				Self::Transition { before, after: *a }
			}
			(Self::Canvas { before, after }, Self::Canvas { before: b, after: a }) if after == *b => {
				Self::Canvas { before, after: *a }
			}
			(Self::Mixes { before, after }, Self::Mixes { before: b, after: a }) if after == *b => Self::Mixes {
				before,
				after: a.clone(),
			},
			_ => return None,
		};
		Some(merged)
	}

	/// Make the change, if `collection` holds what it replaces. Validation is
	/// the caller's, once every change of an edit is made.
	fn apply(&self, collection: &mut Collection) -> Result<(), Error> {
		match self {
			Self::Source { name, before, after } => replace(&mut collection.sources, name, before, after, "source"),
			Self::Audio { name, before, after } => replace(&mut collection.audio, name, before, after, "audio"),
			Self::Video { name, before, after } => replace(&mut collection.video, name, before, after, "video"),
			Self::Scene { index, before, after } => {
				splice(&mut collection.scenes, *index, before, after).map_err(|err| conflict(format!("scene {err}")))
			}
			Self::Item {
				layer,
				index,
				before,
				after,
			} => {
				let items = collection
					.items_mut(layer)
					.ok_or_else(|| conflict(format!("there is no {layer}")))?;
				splice(items, *index, before, after).map_err(|err| conflict(format!("item {err} in {layer}")))
			}
			Self::Transition { before, after } => {
				if collection.transition != *before {
					return Err(conflict("the transition changed".into()));
				}
				collection.transition = *after;
				Ok(())
			}
			Self::Canvas { before, after } => {
				if collection.canvas != *before {
					return Err(conflict("the canvas changed".into()));
				}
				collection.canvas = *after;
				Ok(())
			}
			Self::Mixes { before, after } => {
				if collection.mixes != *before {
					return Err(conflict("the mixes changed".into()));
				}
				collection.mixes = after.clone();
				Ok(())
			}
		}
	}
}

fn conflict(reason: String) -> Error {
	Error::Conflict(reason)
}

fn replace<T: Clone + PartialEq>(
	map: &mut std::collections::BTreeMap<String, T>,
	name: &str,
	before: &Option<T>,
	after: &Option<T>,
	what: &str,
) -> Result<(), Error> {
	if map.get(name) != before.as_ref() {
		return Err(conflict(format!("the {what} entry `{name}` changed")));
	}
	match after {
		Some(after) => map.insert(name.to_string(), after.clone()),
		None => map.remove(name),
	};
	Ok(())
}

/// Insert, remove, or replace `list[index]`, given what it holds now.
fn splice<T: Clone + PartialEq>(
	list: &mut Vec<T>,
	index: usize,
	before: &Option<T>,
	after: &Option<T>,
) -> Result<(), String> {
	match (before, after) {
		(None, Some(after)) if index <= list.len() => list.insert(index, after.clone()),
		(Some(before), _) if list.get(index) == Some(before) => match after {
			Some(after) => list[index] = after.clone(),
			None => {
				list.remove(index);
			}
		},
		_ => return Err(format!("{index} changed")),
	}
	Ok(())
}

/// A labelled set of changes that is applied, undone, and redone whole.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edit {
	/// What the edit does, as the history lists it.
	pub label: String,
	/// Made in order.
	pub changes: Vec<Change>,
}

impl Edit {
	/// The edit that undoes this one.
	pub fn inverse(&self) -> Self {
		Self {
			label: self.label.clone(),
			changes: self.changes.iter().rev().map(Change::inverse).collect(),
		}
	}
}

impl Collection {
	/// Make every change of `edit`, or none: an edit that does not apply to
	/// what the collection holds, or that leaves it invalid, changes nothing.
	pub fn apply(&mut self, edit: &Edit) -> Result<(), Error> {
		let mut next = self.clone();
		for change in &edit.changes {
			change.apply(&mut next)?;
		}
		next.validate()?;
		*self = next;
		Ok(())
	}

	/// What becomes of each scene `edit` renames or removes, by its name here,
	/// before the edit: its new name, or `None` once removed. A scene is
	/// followed by its place in the list rather than by name, so an edit that
	/// reuses a name it just freed, such as renaming `A` to `B` and the old `B`
	/// to `C`, maps each scene to its own new name, and a scene taking a removed
	/// one's name is not mistaken for it. A scene removed and put back unchanged
	/// is the same one moved. Whatever follows a scene by name, such as the one
	/// in preview, looks its name up here once.
	pub fn renames(&self, edit: &Edit) -> BTreeMap<String, Option<String>> {
		// Each place's scene: its name here, or `None` for one the edit added.
		let mut scenes: Vec<(Option<&str>, &str)> = self
			.scenes
			.iter()
			.map(|scene| (Some(scene.name.as_str()), scene.name.as_str()))
			.collect();
		// Scenes the edit removed, with what each was, until one is put back.
		let mut removed: Vec<(Option<&str>, &Scene)> = Vec::new();
		for change in &edit.changes {
			let Change::Scene { index, before, after } = change else {
				continue;
			};
			match (before, after) {
				(None, Some(after)) if *index <= scenes.len() => {
					let back = removed.iter().position(|(_, scene)| *scene == after);
					let was = back.and_then(|back| removed.remove(back).0);
					scenes.insert(*index, (was, &after.name));
				}
				(Some(before), None) if *index < scenes.len() => {
					let (was, _) = scenes.remove(*index);
					removed.push((was, before));
				}
				(Some(_), Some(after)) if *index < scenes.len() => scenes[*index].1 = &after.name,
				_ => {}
			}
		}
		let kept = scenes
			.into_iter()
			.filter_map(|(was, now)| Some((was?, Some(now))))
			.filter(|(was, now)| Some(*was) != *now);
		let gone = removed.into_iter().filter_map(|(was, _)| Some((was?, None)));
		kept.chain(gone)
			.map(|(was, now)| (was.to_string(), now.map(str::to_string)))
			.collect()
	}

	/// Start an edit labelled `label` against this collection.
	pub fn draft(&self, label: impl Into<String>) -> Draft {
		Draft {
			collection: self.clone(),
			edit: Edit {
				label: label.into(),
				changes: Vec::new(),
			},
		}
	}
}

/// An [`Edit`] being built: each change is made to a copy of the collection,
/// so the next one reads what it replaces from the result.
pub struct Draft {
	collection: Collection,
	edit: Edit,
}

impl Draft {
	/// The collection as the changes so far leave it.
	pub fn collection(&self) -> &Collection {
		&self.collection
	}

	/// Make `change` and record it.
	pub fn change(&mut self, change: Change) -> Result<(), Error> {
		change.apply(&mut self.collection)?;
		self.edit.changes.push(change);
		Ok(())
	}

	/// Add, replace, or remove (`None`) the source called `name`, and nothing
	/// else: see [`remove_source`](Self::remove_source) for what refers to it.
	pub fn set_source(&mut self, name: &str, source: Option<Source>) -> Result<(), Error> {
		let before = self.collection.sources.get(name).cloned();
		self.change(Change::Source {
			name: name.to_string(),
			before,
			after: source,
		})
	}

	/// Set or clear the gain, mute, routing, and delay of the source called
	/// `name`.
	pub fn set_audio(&mut self, name: &str, audio: Option<Audio>) -> Result<(), Error> {
		let before = self.collection.audio.get(name).cloned();
		self.change(Change::Audio {
			name: name.to_string(),
			before,
			after: audio,
		})
	}

	/// Set or clear the filters of the source called `name`.
	pub fn set_video(&mut self, name: &str, video: Option<Video>) -> Result<(), Error> {
		let before = self.collection.video.get(name).cloned();
		let after = video.filter(|video| *video != Video::default());
		self.change(Change::Video {
			name: name.to_string(),
			before,
			after,
		})
	}

	/// Remove the source called `name` and everything that refers to it: its
	/// clones, the items showing either, their audio and video settings, and
	/// the filters reading it, such as a mask.
	pub fn remove_source(&mut self, name: &str) -> Result<(), Error> {
		let clones: Vec<String> = self
			.collection
			.sources
			.iter()
			.filter(|(_, source)| matches!(source, Source::Clone { source } if source == name))
			.map(|(clone, _)| clone.clone())
			.collect();
		for clone in clones {
			self.remove_source(&clone)?;
		}

		let placed: Vec<(Layer, usize)> = self
			.collection
			.layers()
			.flat_map(|(layer, items)| {
				let at = items.iter().enumerate().filter(|(_, item)| item.source == name);
				at.map(move |(index, _)| (layer.clone(), index)).collect::<Vec<_>>()
			})
			.collect();
		// Back to front, so each index still names the item found.
		for (layer, index) in placed.into_iter().rev() {
			self.remove_item(&layer, index)?;
		}

		let reads = |filter: &Filter| filter.source() == Some(name);
		let items: Vec<(Layer, usize, Item)> = self
			.collection
			.layers()
			.flat_map(|(layer, items)| {
				let reading = items
					.iter()
					.enumerate()
					.filter(|(_, item)| item.filters.iter().any(reads));
				reading
					.map(move |(index, item)| (layer.clone(), index, item.clone()))
					.collect::<Vec<_>>()
			})
			.collect();
		for (layer, index, mut item) in items {
			item.filters.retain(|filter| !reads(filter));
			self.set_item(&layer, index, item)?;
		}
		let videos: Vec<(String, Video)> = self
			.collection
			.video
			.iter()
			.filter(|(_, video)| video.filters.iter().any(reads))
			.map(|(name, video)| (name.clone(), video.clone()))
			.collect();
		for (other, mut video) in videos {
			video.filters.retain(|filter| !reads(filter));
			self.set_video(&other, Some(video))?;
		}
		if self.collection.audio.contains_key(name) {
			self.set_audio(name, None)?;
		}
		if self.collection.video.contains_key(name) {
			self.set_video(name, None)?;
		}
		self.set_source(name, None)
	}

	/// Insert `scene` at `index`.
	pub fn insert_scene(&mut self, index: usize, scene: Scene) -> Result<(), Error> {
		self.change(Change::Scene {
			index,
			before: None,
			after: Some(scene),
		})
	}

	/// Remove the scene at `index`, with its items.
	pub fn remove_scene(&mut self, index: usize) -> Result<(), Error> {
		let before = self.scene_at(index)?;
		self.change(Change::Scene {
			index,
			before: Some(before),
			after: None,
		})
	}

	/// Replace the scene at `index`.
	pub fn set_scene(&mut self, index: usize, scene: Scene) -> Result<(), Error> {
		let before = self.scene_at(index)?;
		self.change(Change::Scene {
			index,
			before: Some(before),
			after: Some(scene),
		})
	}

	/// Rename the scene at `index`, keeping what it shows.
	pub fn rename_scene(&mut self, index: usize, name: impl Into<String>) -> Result<(), Error> {
		let scene = Scene {
			name: name.into(),
			..self.scene_at(index)?
		};
		self.set_scene(index, scene)
	}

	/// Move the scene at `from` to `to`, counted after its removal.
	pub fn move_scene(&mut self, from: usize, to: usize) -> Result<(), Error> {
		let scene = self.scene_at(from)?;
		self.remove_scene(from)?;
		self.insert_scene(to, scene)
	}

	fn scene_at(&self, index: usize) -> Result<Scene, Error> {
		let scene = self.collection.scenes.get(index).cloned();
		scene.ok_or_else(|| conflict(format!("there is no scene {index}")))
	}

	/// Insert `item` at `index` of `layer`.
	pub fn insert_item(&mut self, layer: &Layer, index: usize, item: Item) -> Result<(), Error> {
		self.change(Change::Item {
			layer: layer.clone(),
			index,
			before: None,
			after: Some(item),
		})
	}

	/// Remove the item at `index` of `layer`, returning it.
	pub fn remove_item(&mut self, layer: &Layer, index: usize) -> Result<Item, Error> {
		let before = self.item_at(layer, index)?;
		self.change(Change::Item {
			layer: layer.clone(),
			index,
			before: Some(before.clone()),
			after: None,
		})?;
		Ok(before)
	}

	/// Replace the item at `index` of `layer`.
	pub fn set_item(&mut self, layer: &Layer, index: usize, item: Item) -> Result<(), Error> {
		let before = self.item_at(layer, index)?;
		self.change(Change::Item {
			layer: layer.clone(),
			index,
			before: Some(before),
			after: Some(item),
		})
	}

	/// Move the item at `from` of `layer` to `to`, counted after its removal.
	pub fn move_item(&mut self, layer: &Layer, from: usize, to: usize) -> Result<(), Error> {
		let item = self.remove_item(layer, from)?;
		self.insert_item(layer, to, item)
	}

	fn item_at(&self, layer: &Layer, index: usize) -> Result<Item, Error> {
		let item = self.collection.items(layer).and_then(|items| items.get(index)).cloned();
		item.ok_or_else(|| conflict(format!("there is no item {index} in {layer}")))
	}

	/// Set how a fade behaves.
	pub fn set_transition(&mut self, transition: Transition) -> Result<(), Error> {
		let before = self.collection.transition;
		self.change(Change::Transition {
			before,
			after: transition,
		})
	}

	/// Set the canvas's size and frame rate. Items keep their place in canvas
	/// pixels.
	pub fn set_canvas(&mut self, canvas: Canvas) -> Result<(), Error> {
		let before = self.collection.canvas;
		self.change(Change::Canvas { before, after: canvas })
	}

	/// Set the mixes and which one the program carries, and nothing else: see
	/// [`rename_mix`](Self::rename_mix) and [`remove_mix`](Self::remove_mix) for
	/// what refers to a mix.
	pub fn set_mixes(&mut self, mixes: Mixes) -> Result<(), Error> {
		let before = self.collection.mixes.clone();
		self.change(Change::Mixes { before, after: mixes })
	}

	/// Rename the mix `from` to `to`, keeping its place, its routing, and
	/// whether the program carries it.
	pub fn rename_mix(&mut self, from: &str, to: &str) -> Result<(), Error> {
		self.retarget_mix(from, Some(to))
	}

	/// Remove the mix `name`, and every source's exclusion from it. The mix the
	/// program carries cannot be removed.
	pub fn remove_mix(&mut self, name: &str) -> Result<(), Error> {
		self.retarget_mix(name, None)
	}

	/// Rename the mix `from` to `to`, or remove it with `None`, and every
	/// reference to it.
	fn retarget_mix(&mut self, from: &str, to: Option<&str>) -> Result<(), Error> {
		let mut mixes = self.collection.mixes.clone();
		let index = mixes.names.iter().position(|name| name == from);
		let index = index.ok_or_else(|| conflict(format!("there is no mix `{from}`")))?;
		match to {
			Some(to) => mixes.names[index] = to.to_string(),
			None => {
				mixes.names.remove(index);
			}
		}
		if mixes.program == from {
			mixes.program = to.unwrap_or_default().to_string();
		}
		let routed: Vec<(String, Audio)> = self
			.collection
			.audio
			.iter()
			.filter(|(_, audio)| audio.exclude.contains(from))
			.map(|(name, audio)| (name.clone(), audio.clone()))
			.collect();
		self.set_mixes(mixes)?;
		for (name, mut audio) in routed {
			audio.exclude.remove(from);
			if let Some(to) = to {
				audio.exclude.insert(to.to_string());
			}
			self.set_audio(&name, Some(audio))?;
		}
		Ok(())
	}

	/// The edit, once the collection it leaves is valid. Changes that ended up
	/// replacing something with itself are left out.
	pub fn finish(mut self) -> Result<Edit, Error> {
		self.collection.validate()?;
		self.edit.changes.retain(|change| !change.is_noop());
		Ok(self.edit)
	}
}

/// What a collection was edited by, to undo and redo.
///
/// Holds at most [`LIMIT`](Self::LIMIT) edits, dropping the oldest. Saved
/// with [`to_toml`](Self::to_toml) beside the scene file and loaded with
/// [`from_toml`](Self::from_toml), which keeps only the entries that still
/// replay against the collection the file holds.
#[derive(Clone, Debug, Default)]
pub struct History {
	entries: VecDeque<Entry>,
	/// How many entries, from the oldest, the collection reflects; the rest
	/// can be redone.
	position: usize,
}

#[derive(Clone, Debug)]
struct Entry {
	edit: Edit,
	/// The interaction that made it, so a drag is one entry.
	gesture: Option<u64>,
}

/// The saved form of a [`History`].
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
	position: usize,
	#[serde(default)]
	entries: Vec<Edit>,
}

impl History {
	/// The most edits a history holds.
	pub const LIMIT: usize = 100;

	/// Nothing to undo or redo.
	pub fn new() -> Self {
		Self::default()
	}

	/// Every edit, oldest first.
	pub fn entries(&self) -> impl ExactSizeIterator<Item = &Edit> {
		self.entries.iter().map(|entry| &entry.edit)
	}

	/// How many [`entries`](Self::entries), from the oldest, the collection
	/// reflects. The rest can be redone.
	pub fn position(&self) -> usize {
		self.position
	}

	/// Record `edit`, which was just applied, dropping whatever could be redone.
	///
	/// An edit carrying the `gesture` of the newest entry, such as the next
	/// step of the same drag, joins that entry when both change the same one
	/// part, so the gesture undoes at once.
	pub fn record(&mut self, edit: Edit, gesture: Option<u64>) {
		if edit.changes.is_empty() {
			return;
		}
		self.entries.truncate(self.position);
		if let Some(gesture) = gesture
			&& let Some(last) = self.entries.back_mut()
			&& last.gesture == Some(gesture)
			&& let ([older], [newer]) = (last.edit.changes.as_slice(), edit.changes.as_slice())
			&& let Some(merged) = older.merge(newer)
		{
			if merged.is_noop() {
				self.entries.pop_back();
			} else {
				last.edit.changes = vec![merged];
			}
			self.position = self.entries.len();
			return;
		}

		self.entries.push_back(Entry { edit, gesture });
		while self.entries.len() > Self::LIMIT {
			self.entries.pop_front();
		}
		self.position = self.entries.len();
	}

	/// Undo the newest edit `collection` reflects. Returns it, or `None` when
	/// there is nothing to undo.
	pub fn undo(&mut self, collection: &mut Collection) -> Result<Option<&Edit>, Error> {
		if self.position == 0 {
			return Ok(None);
		}
		self.seek(collection, self.position - 1)?;
		Ok(Some(&self.entries[self.position].edit))
	}

	/// Redo the oldest edit undone. Returns it, or `None` when there is
	/// nothing to redo.
	pub fn redo(&mut self, collection: &mut Collection) -> Result<Option<&Edit>, Error> {
		if self.position == self.entries.len() {
			return Ok(None);
		}
		self.seek(collection, self.position + 1)?;
		Ok(Some(&self.entries[self.position - 1].edit))
	}

	/// Undo or redo until `collection` reflects the oldest `position` entries,
	/// all at once or not at all.
	pub fn seek(&mut self, collection: &mut Collection, position: usize) -> Result<(), Error> {
		if position > self.entries.len() {
			return Err(conflict(format!("the history has no entry {position}")));
		}
		let mut next = collection.clone();
		if position < self.position {
			for entry in self.entries.range(position..self.position).rev() {
				next.apply(&entry.edit.inverse())?;
			}
		} else {
			for entry in self.entries.range(self.position..position) {
				next.apply(&entry.edit)?;
			}
		}
		*collection = next;
		self.position = position;
		// A gesture continues only the entry it made.
		for entry in &mut self.entries {
			entry.gesture = None;
		}
		Ok(())
	}

	/// Make `edit` to `collection` without recording it, for a change the
	/// user did not make, such as a guest joining. It is not undone, and every
	/// edit here still undoes and redoes around it.
	///
	/// So it must commute with each of them: it may only add source, audio,
	/// and video entries, under names no edit here changes, and nothing that
	/// names another source, such as a clone or a mask, which an edit could
	/// remove. Anything else is refused.
	pub fn apply_outside(&self, collection: &mut Collection, edit: &Edit) -> Result<(), Error> {
		for change in &edit.changes {
			let name = match change {
				Change::Source {
					name,
					before: None,
					after: Some(source),
				} if source.opens() => name,
				Change::Audio {
					name,
					before: None,
					after: Some(_),
				} => name,
				Change::Video {
					name,
					before: None,
					after: Some(video),
				} if video.filters.iter().all(|filter| filter.source().is_none()) => name,
				_ => {
					return Err(Error::Invalid(format!(
						"`{}` changes more than adding a source's own entries",
						edit.label
					)));
				}
			};
			if self.touches(name) {
				return Err(conflict(format!("the history edits `{name}`")));
			}
		}
		collection.apply(edit)
	}

	/// Whether any edit here, done or undone, changes the source, audio, or
	/// video entry called `name`.
	pub fn touches(&self, name: &str) -> bool {
		let mut changes = self.entries().flat_map(|edit| &edit.changes);
		changes.any(|change| change.entry() == Some(name))
	}

	/// `base`, or `base 2`, `base 3`, and so on: the first name no source of
	/// `collection` has and no edit here changes, for
	/// [`apply_outside`](Self::apply_outside) to add.
	pub fn unused_name(&self, collection: &Collection, base: &str) -> String {
		std::iter::once(base.to_string())
			.chain((2..).map(|n| format!("{base} {n}")))
			.find(|name| !collection.sources.contains_key(name) && !self.touches(name))
			.expect("an unused name")
	}

	/// Serialize to the history file format, in the scene file's version,
	/// since entries hold its sources, scenes, and items.
	pub fn to_toml(&self) -> Result<String, Error> {
		let file = File {
			position: self.position,
			entries: self.entries().cloned().collect(),
		};
		let body = toml::to_string(&file).map_err(|err| Error::Format(err.to_string()))?;
		Ok(format!("version = {}\n\n{body}", Collection::VERSION))
	}

	/// Parse a history file saved beside the scene file that holds `collection`.
	///
	/// Keeps the entries that still replay against `collection`: an entry to
	/// undo that no longer applies, such as after a hand edit of the scene
	/// file, is dropped with every older one, and an entry to redo that no
	/// longer applies is dropped with every newer one. Returns the history and
	/// how many entries were dropped. A file that is not a history of this
	/// version fails.
	pub fn from_toml(text: &str, collection: &Collection) -> Result<(Self, usize), Error> {
		let table = crate::collection::versioned(text)?;
		let file = File::deserialize(table).map_err(|err| Error::Format(err.to_string()))?;
		if file.position > file.entries.len() {
			return Err(Error::Format(format!(
				"position {} is past the history's {} entries",
				file.position,
				file.entries.len()
			)));
		}
		let total = file.entries.len();
		let (undo, redo) = file.entries.split_at(file.position);

		// Newest first, back to the first that no longer applies.
		let mut state = collection.clone();
		let undo = undo
			.iter()
			.rev()
			.take(Self::LIMIT)
			.take_while(|edit| state.apply(&edit.inverse()).is_ok())
			.collect::<Vec<_>>();
		let mut state = collection.clone();
		let redo = redo
			.iter()
			.take(Self::LIMIT - undo.len())
			.take_while(|edit| state.apply(edit).is_ok())
			.collect::<Vec<_>>();

		let entry = |edit: &Edit| Entry {
			edit: edit.clone(),
			gesture: None,
		};
		let history = Self {
			position: undo.len(),
			entries: undo.iter().rev().chain(&redo).map(|edit| entry(edit)).collect(),
		};
		let dropped = total - history.entries.len();
		Ok((history, dropped))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::collection::{Filter, Rect, Rgba};

	fn color(rgba: [u8; 4]) -> Source {
		Source::Color { color: Rgba(rgba) }
	}

	fn main() -> Layer {
		Layer::Scene("Scene".into())
	}

	/// A collection with a red source shown in the one scene.
	fn collection() -> Collection {
		let mut collection = Collection::new();
		collection.sources.insert("red".into(), color([255, 0, 0, 255]));
		collection.scenes[0]
			.items
			.push(Item::new("red", Rect::fill(&collection.canvas)));
		collection
	}

	fn commit(collection: &mut Collection, history: &mut History, draft: Draft, gesture: Option<u64>) {
		let edit = draft.finish().unwrap();
		collection.apply(&edit).unwrap();
		history.record(edit, gesture);
	}

	/// Move red to `x`.
	fn drag(collection: &mut Collection, history: &mut History, x: f32, gesture: Option<u64>) {
		let mut draft = collection.draft("Move red");
		let mut item = collection.scenes[0].items[0].clone();
		item.rect.x = x;
		draft.set_item(&main(), 0, item).unwrap();
		commit(collection, history, draft, gesture);
	}

	/// Undo walks back through every edit to the start and redo walks forward
	/// again, and a new edit drops what could be redone.
	#[test]
	fn undoes_and_redoes_every_edit() {
		let start = collection();
		let mut collection = start.clone();
		let mut history = History::new();

		let mut draft = collection.draft("Add blue");
		draft.set_source("blue", Some(color([0, 0, 255, 255]))).unwrap();
		draft
			.insert_item(&main(), 1, Item::new("blue", Rect::fill(&collection.canvas)))
			.unwrap();
		commit(&mut collection, &mut history, draft, None);
		let added = collection.clone();
		drag(&mut collection, &mut history, 10.0, None);
		let moved_once = collection.clone();

		assert_eq!(history.undo(&mut collection).unwrap().unwrap().label, "Move red");
		assert_eq!(collection, added);
		assert_eq!(history.undo(&mut collection).unwrap().unwrap().label, "Add blue");
		assert_eq!(collection, start);
		assert!(history.undo(&mut collection).unwrap().is_none());

		history.redo(&mut collection).unwrap();
		history.redo(&mut collection).unwrap();
		assert_eq!(collection, moved_once);
		assert!(history.redo(&mut collection).unwrap().is_none());

		history.seek(&mut collection, 1).unwrap();
		drag(&mut collection, &mut history, 20.0, None);
		assert_eq!(history.entries().len(), 2);
		assert!(history.redo(&mut collection).unwrap().is_none());
	}

	/// Removing a source removes its clones, every item showing either, and
	/// their settings, and one undo puts all of it back.
	#[test]
	fn removing_a_source_cascades_and_undoes_whole() {
		let mut collection = collection();
		collection
			.sources
			.insert("red copy".into(), Source::Clone { source: "red".into() });
		collection.video.insert(
			"red copy".into(),
			Video {
				filters: vec![Filter::Opacity { amount: 0.5 }],
				..Default::default()
			},
		);
		collection
			.top
			.push(Item::new("red copy", Rect::fill(&collection.canvas)));
		collection.scenes[0]
			.items
			.push(Item::new("red", Rect::fill(&collection.canvas)));
		collection.validate().unwrap();
		let before = collection.clone();

		let mut history = History::new();
		let mut draft = collection.draft("Remove red");
		draft.remove_source("red").unwrap();
		commit(&mut collection, &mut history, draft, None);
		assert!(collection.sources.is_empty());
		assert!(collection.video.is_empty());
		assert!(collection.top.is_empty());
		assert!(collection.scenes[0].items.is_empty());

		history.undo(&mut collection).unwrap();
		assert_eq!(collection, before);
	}

	/// A removed source's filters go with it, from items and other sources
	/// alike, leaving their other filters.
	#[test]
	fn removing_a_source_removes_the_filters_reading_it() {
		let mut collection = collection();
		collection.sources.insert("matte".into(), color([0, 0, 0, 128]));
		collection.sources.insert(
			"grade".into(),
			Source::Lut {
				path: "grade.cube".into(),
			},
		);
		let mask = Filter::Mask {
			source: "matte".into(),
			mode: Default::default(),
		};
		let lut = Filter::Lut {
			source: "grade".into(),
			amount: 1.0,
		};
		collection.scenes[0].items[0].filters = vec![mask.clone(), lut.clone()];
		collection.video.insert(
			"red".into(),
			Video {
				filters: vec![lut.clone(), mask],
				..Default::default()
			},
		);
		collection.validate().unwrap();

		let mut draft = collection.draft("Remove matte");
		draft.remove_source("matte").unwrap();
		collection.apply(&draft.finish().unwrap()).unwrap();
		assert_eq!(collection.scenes[0].items[0].filters, std::slice::from_ref(&lut));
		assert_eq!(collection.video["red"].filters, [lut]);
	}

	/// An edit that leaves the collection invalid is refused whole.
	#[test]
	fn refuses_an_edit_that_leaves_it_invalid() {
		let collection = collection();
		let mut draft = collection.draft("Remove red alone");
		draft.set_source("red", None).unwrap();
		assert!(matches!(draft.finish(), Err(Error::Invalid(_))));

		let edit = Edit {
			label: "Dangling".into(),
			changes: vec![Change::Source {
				name: "red".into(),
				before: Some(color([255, 0, 0, 255])),
				after: None,
			}],
		};
		let mut applied = collection.clone();
		assert!(applied.apply(&edit).is_err());
		assert_eq!(applied, collection, "nothing changed");
	}

	/// The steps of one drag are one entry, a later drag is another, and a drag
	/// back to where it started leaves nothing to undo.
	#[test]
	fn a_gesture_is_one_entry() {
		let mut collection = collection();
		let mut history = History::new();
		for x in [1.0, 2.0, 3.0] {
			drag(&mut collection, &mut history, x, Some(7));
		}
		assert_eq!(history.entries().len(), 1);
		for x in [4.0, 5.0] {
			drag(&mut collection, &mut history, x, Some(8));
		}
		assert_eq!(history.entries().len(), 2);
		history.undo(&mut collection).unwrap();
		assert_eq!(collection.scenes[0].items[0].rect.x, 3.0);

		// An undo ends the gesture, so the same id starts a new entry.
		drag(&mut collection, &mut history, 6.0, Some(7));
		assert_eq!(history.entries().len(), 2);

		drag(&mut collection, &mut history, 9.0, Some(9));
		drag(&mut collection, &mut history, 6.0, Some(9));
		assert_eq!(history.entries().len(), 2, "a drag back to the start is no edit");
	}

	fn renamed(pairs: &[(&str, Option<&str>)]) -> BTreeMap<String, Option<String>> {
		pairs
			.iter()
			.map(|(was, now)| (was.to_string(), now.map(str::to_string)))
			.collect()
	}

	/// A rename keeps the scene's items and place, a move keeps its name, and
	/// each scene maps to its own new name, even through names the edit
	/// frees and reuses.
	#[test]
	fn renames_and_reorders_scenes() {
		let mut collection = collection();
		collection.scenes.push(Scene::new("Other"));
		let start = collection.clone();
		let mut history = History::new();

		let mut draft = collection.draft("Swap names");
		draft.rename_scene(0, "swap").unwrap();
		draft.rename_scene(1, "Scene").unwrap();
		draft.rename_scene(0, "Other").unwrap();
		let edit = draft.finish().unwrap();
		let renames = collection.renames(&edit);
		assert_eq!(renames, renamed(&[("Scene", Some("Other")), ("Other", Some("Scene"))]));
		collection.apply(&edit).unwrap();
		history.record(edit, None);
		assert_eq!(collection.scenes[0].name, "Other");
		assert_eq!(collection.scenes[0].items, start.scenes[0].items);

		let mut draft = collection.draft("Move `Other` down");
		draft.move_scene(0, 1).unwrap();
		let edit = draft.finish().unwrap();
		assert!(collection.renames(&edit).is_empty(), "a move is no rename");
		collection.apply(&edit).unwrap();
		history.record(edit, None);
		let names: Vec<_> = collection.scenes.iter().map(|scene| scene.name.as_str()).collect();
		assert_eq!(names, ["Scene", "Other"]);
		assert_eq!(collection.scenes[1].items, start.scenes[0].items);

		history.seek(&mut collection, 0).unwrap();
		assert_eq!(collection, start);

		// The second rename takes the name the first freed, so read one after
		// the other, `Scene` would land on `Next`, the other scene.
		let mut draft = collection.draft("Chain");
		draft.rename_scene(0, "Other").unwrap();
		draft.rename_scene(1, "Next").unwrap();
		draft.insert_scene(0, Scene::new("New")).unwrap();
		let edit = draft.finish().unwrap();
		assert_eq!(
			collection.renames(&edit),
			renamed(&[("Scene", Some("Other")), ("Other", Some("Next"))]),
			"an added scene renames nothing"
		);
		collection.apply(&edit).unwrap();
		assert_eq!(
			collection.renames(&edit.inverse()),
			renamed(&[("Other", Some("Scene")), ("Next", Some("Other")), ("New", None)]),
			"the undo maps each back and removes the added scene"
		);

		// A removed scene maps to nothing, even when another takes its name.
		let mut draft = collection.draft("Replace");
		draft.remove_scene(1).unwrap();
		draft.rename_scene(1, "Other").unwrap();
		let edit = draft.finish().unwrap();
		assert_eq!(
			collection.renames(&edit),
			renamed(&[("Other", None), ("Next", Some("Other"))])
		);

		// Names stay unique and non-empty.
		for name in ["Other", ""] {
			let mut draft = collection.draft("Rename");
			draft.rename_scene(0, name).unwrap();
			assert!(matches!(draft.finish(), Err(Error::Invalid(_))), "{name:?}");
		}
	}

	/// The canvas and the transition are edits like any other: a drag of one
	/// undoes as one step, and a canvas the encoder cannot take is refused.
	#[test]
	fn the_canvas_and_transition_undo() {
		let mut collection = collection();
		let start = collection.clone();
		let mut history = History::new();
		for millis in [400, 500, 600] {
			let mut draft = collection.draft("Set the fade time");
			draft
				.set_transition(Transition {
					fade: std::time::Duration::from_millis(millis),
				})
				.unwrap();
			commit(&mut collection, &mut history, draft, Some(1));
		}
		let canvas = Canvas {
			width: 1280,
			height: 720,
			framerate: 60,
		};
		let mut draft = collection.draft("Resize the canvas");
		draft.set_canvas(canvas).unwrap();
		commit(&mut collection, &mut history, draft, None);
		assert_eq!(history.entries().len(), 2);
		assert_eq!(collection.canvas, canvas);
		assert_eq!(
			collection.scenes[0].items[0].rect,
			Rect::fill(&start.canvas),
			"items keep their pixels"
		);

		history.undo(&mut collection).unwrap();
		assert_eq!(collection.canvas, start.canvas);
		history.undo(&mut collection).unwrap();
		assert_eq!(collection, start);

		let mut draft = collection.draft("Resize the canvas");
		draft.set_canvas(Canvas { width: 1279, ..canvas }).unwrap();
		assert!(matches!(draft.finish(), Err(Error::Invalid(_))));
	}

	/// A renamed mix keeps its place, the program's pick, and every source left
	/// out of it; a removed one takes those exclusions with it and undoes
	/// whole; and the mix the program carries stays.
	#[test]
	fn a_mix_renames_and_removes_with_its_routing() {
		let mut collection = collection();
		collection.sources.insert(
			"guest".into(),
			Source::Broadcast {
				path: "room/guest".into(),
			},
		);
		collection.mixes = Mixes {
			names: vec!["Program".into(), "Stream".into()],
			program: "Stream".into(),
		};
		let left_out = Audio {
			exclude: ["Stream".to_string()].into(),
			..Audio::default()
		};
		collection.audio.insert("guest".into(), left_out);
		collection.validate().unwrap();
		let start = collection.clone();
		let mut history = History::new();

		let mut draft = collection.draft("Rename mix");
		draft.rename_mix("Stream", "Live").unwrap();
		commit(&mut collection, &mut history, draft, None);
		assert_eq!(collection.mixes.names, ["Program", "Live"]);
		assert_eq!(collection.mixes.program, "Live");
		assert_eq!(collection.audio["guest"].exclude, ["Live".to_string()].into());

		let mut draft = collection.draft("Remove mix");
		draft.remove_mix("Live").unwrap();
		assert!(
			matches!(draft.finish(), Err(Error::Invalid(_))),
			"the program carries it"
		);

		let mut draft = collection.draft("Pick mix");
		let mixes = Mixes {
			program: "Program".into(),
			..collection.mixes.clone()
		};
		draft.set_mixes(mixes).unwrap();
		draft.remove_mix("Live").unwrap();
		commit(&mut collection, &mut history, draft, None);
		assert_eq!(collection.mixes, Mixes::default());
		assert!(collection.audio["guest"].exclude.is_empty());

		history.undo(&mut collection).unwrap();
		history.undo(&mut collection).unwrap();
		assert_eq!(collection, start);
	}

	#[test]
	fn holds_at_most_the_limit() {
		let mut collection = collection();
		let mut history = History::new();
		for x in 0..History::LIMIT + 5 {
			drag(&mut collection, &mut history, x as f32 + 1.0, None);
		}
		assert_eq!(history.entries().len(), History::LIMIT);
		assert_eq!(history.position(), History::LIMIT);
		while history.undo(&mut collection).unwrap().is_some() {}
		assert_eq!(collection.scenes[0].items[0].rect.x, 5.0);
	}

	/// A saved history reloads against the collection saved with it, undo and
	/// redo intact.
	#[test]
	fn round_trips() {
		let mut collection = collection();
		let mut history = History::new();
		for x in [1.0, 2.0, 3.0] {
			drag(&mut collection, &mut history, x, None);
		}
		let mut draft = collection.draft("Remove red");
		draft.remove_source("red").unwrap();
		commit(&mut collection, &mut history, draft, None);
		history.undo(&mut collection).unwrap();

		let text = history.to_toml().unwrap();
		assert!(text.starts_with("version = 1\n"), "{text}");
		let (mut loaded, dropped) = History::from_toml(&text, &collection).unwrap();
		assert_eq!(dropped, 0);
		assert_eq!(loaded.position(), 3);
		assert!(loaded.entries().eq(history.entries()));

		loaded.redo(&mut collection).unwrap();
		assert!(collection.sources.is_empty());
	}

	/// After a hand edit, an entry the scene file no longer matches is dropped
	/// with everything older, and the newer ones still undo.
	#[test]
	fn drops_what_no_longer_replays() {
		let mut collection = collection();
		let mut history = History::new();
		drag(&mut collection, &mut history, 1.0, None);
		let mut draft = collection.draft("Add blue");
		draft.set_source("blue", Some(color([0, 0, 255, 255]))).unwrap();
		commit(&mut collection, &mut history, draft, None);
		let mut draft = collection.draft("Add green");
		draft.set_source("green", Some(color([0, 255, 0, 255]))).unwrap();
		commit(&mut collection, &mut history, draft, None);
		history.undo(&mut collection).unwrap();
		let text = history.to_toml().unwrap();

		// By hand: red moved again, and green added, so neither the first
		// entry nor the one to redo replays.
		let mut edited = collection.clone();
		edited.scenes[0].items[0].rect.x = 50.0;
		edited.sources.insert("green".into(), color([0, 255, 0, 255]));
		let (mut loaded, dropped) = History::from_toml(&text, &edited).unwrap();
		assert_eq!(dropped, 2);
		assert_eq!(loaded.position(), 1);
		assert_eq!(loaded.entries().next().unwrap().label, "Add blue");
		loaded.undo(&mut edited).unwrap();
		assert!(!edited.sources.contains_key("blue"));
	}

	/// A change from outside, such as a guest joining, drops nothing to redo,
	/// and undo and redo pass over it, here or after a reload.
	#[test]
	fn an_outside_edit_keeps_the_history_around_it() {
		let mut collection = collection();
		let mut history = History::new();
		drag(&mut collection, &mut history, 1.0, None);
		drag(&mut collection, &mut history, 2.0, None);
		history.undo(&mut collection).unwrap();

		let mut draft = collection.draft("Add guest");
		draft.set_source("guest", Some(color([0, 255, 0, 255]))).unwrap();
		draft.set_audio("guest", Some(Audio::default())).unwrap();
		history
			.apply_outside(&mut collection, &draft.finish().unwrap())
			.unwrap();
		assert_eq!((history.position(), history.entries().len()), (1, 2));

		history.redo(&mut collection).unwrap();
		assert_eq!(collection.scenes[0].items[0].rect.x, 2.0);
		history.seek(&mut collection, 0).unwrap();
		assert_eq!(collection.scenes[0].items[0].rect.x, 0.0);
		assert!(collection.sources.contains_key("guest"));

		// The scene file now holds the guest, and the whole history still replays.
		let (mut loaded, dropped) = History::from_toml(&history.to_toml().unwrap(), &collection).unwrap();
		assert_eq!(dropped, 0);
		loaded.seek(&mut collection, 2).unwrap();
		assert_eq!(collection.scenes[0].items[0].rect.x, 2.0);
		assert!(collection.sources.contains_key("guest"));
	}

	/// An outside change that an edit's undo or redo would collide with, or
	/// that moves what an edit names by index, is refused.
	#[test]
	fn refuses_an_outside_edit_the_history_could_not_pass_over() {
		let mut collection = collection();
		let mut history = History::new();
		let mut draft = collection.draft("Add blue");
		draft.set_source("blue", Some(color([0, 0, 255, 255]))).unwrap();
		commit(&mut collection, &mut history, draft, None);
		history.undo(&mut collection).unwrap();
		let before = collection.clone();

		let mut taken = collection.draft("Add guest");
		taken.set_source("blue", Some(color([0, 0, 255, 255]))).unwrap();
		let mut clone = collection.draft("Add guest");
		clone
			.set_source("clone", Some(Source::Clone { source: "red".into() }))
			.unwrap();
		let mut placed = collection.draft("Add guest");
		placed.set_source("guest", Some(color([0, 255, 0, 255]))).unwrap();
		let item = Item::new("guest", Rect::fill(&collection.canvas));
		placed.insert_item(&main(), 0, item).unwrap();
		let mut masked = collection.draft("Add guest");
		masked.set_source("guest", Some(color([0, 255, 0, 255]))).unwrap();
		let mask = Filter::Mask {
			source: "red".into(),
			mode: Default::default(),
		};
		let video = Video {
			filters: vec![mask],
			..Default::default()
		};
		masked.set_video("guest", Some(video)).unwrap();
		let mut removed = collection.draft("Remove red");
		removed.remove_source("red").unwrap();
		for draft in [taken, clone, placed, masked, removed] {
			let edit = draft.finish().unwrap();
			assert!(history.apply_outside(&mut collection, &edit).is_err(), "{edit:?}");
		}
		assert_eq!(collection, before);
		assert_eq!(history.unused_name(&collection, "blue"), "blue 2");
		assert_eq!(history.unused_name(&collection, "red"), "red 2");
	}

	#[test]
	fn refuses_a_file_that_is_not_a_history() {
		let collection = collection();
		for text in [
			"",
			"version = 2\nposition = 0",
			"version = 1\nposition = 1",
			"version = 1",
		] {
			assert!(History::from_toml(text, &collection).is_err(), "{text}");
		}
	}
}
