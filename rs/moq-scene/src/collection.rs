//! The scene graph, and the TOML file a [`Collection`] saves to.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_with::{DeserializeFromStr, SerializeDisplay};

use crate::Error;

/// Everything a studio composes: the program's canvas, the sources, and the
/// scenes that place them.
///
/// Saved with [`to_toml`](Self::to_toml) and loaded with
/// [`from_toml`](Self::from_toml). The file names devices, files, and broadcast
/// paths, never a relay URL or a token, so it is safe to commit and share.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Collection {
	/// The program's size and frame rate: the size of every scene that sets
	/// none, and the frame rate every scene is drawn at.
	pub canvas: Canvas,
	/// How the preview is taken to program, short of a cut.
	#[serde(default)]
	pub transition: Transition,
	/// Every source, by its unique name. Scenes refer to sources by that name,
	/// so one camera can appear in many scenes while being opened once.
	#[serde(default)]
	pub sources: BTreeMap<String, Source>,
	/// The gain, mute, and sync delay of each source with sound, by source
	/// name, in every audio scene that holds it. A source without an entry
	/// plays at unity.
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub audio: BTreeMap<String, Audio>,
	/// The filters on each picture-carrying source, by source name, applied
	/// wherever the source is placed. A source without an entry is unfiltered.
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub video: BTreeMap<String, Video>,
	/// The sources published independently, by name: each one's own track
	/// goes to the relay beside the program, for viewers and services to
	/// place themselves, and the source is left out of every picture and mix
	/// the studio composites. Only a source with a track of its own can be,
	/// and not one with its background removed, since its track would show
	/// the background.
	#[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
	pub published: BTreeSet<String>,
	/// The scenes, in the order the scene list shows them.
	#[serde(default)]
	pub scenes: Vec<Scene>,
	/// The audio scenes, in the order the scene list shows them after the
	/// scenes: one on program, and any others for recordings, published
	/// scenes, headphones, or to nest. Independent of cuts.
	#[serde(default = "AudioScene::defaults", skip_serializing_if = "AudioScene::are_default")]
	pub audio_scenes: Vec<AudioScene>,
	/// The top layer: items drawn over whichever scene is on program, and in
	/// preview over one of the program's size, back to front, so they stay on
	/// across scene switches (a downstream keyer). They do not fade with a
	/// transition.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub top: Vec<Item>,
}

impl Collection {
	/// The scene file format version this build reads and writes.
	///
	/// A file names its version, and loading any other one fails rather than
	/// guessing at fields whose meaning changed.
	pub const VERSION: u32 = 1;

	/// An empty collection with one blank scene on a 1080p, 30 fps canvas.
	pub fn new() -> Self {
		Self {
			canvas: Canvas::default(),
			transition: Transition::default(),
			sources: BTreeMap::new(),
			audio: BTreeMap::new(),
			video: BTreeMap::new(),
			published: BTreeSet::new(),
			scenes: vec![Scene::new("Scene")],
			audio_scenes: AudioScene::defaults(),
			top: Vec::new(),
		}
	}

	/// Parse and validate a scene file.
	pub fn from_toml(text: &str) -> Result<Self, Error> {
		let table = versioned(text)?;
		let collection = Self::deserialize(table).map_err(|err| Error::Format(err.to_string()))?;
		collection.validate()?;
		Ok(collection)
	}

	/// Serialize to the scene file format, `version` first.
	pub fn to_toml(&self) -> Result<String, Error> {
		self.validate()?;
		let body = toml::to_string(self).map_err(|err| Error::Format(err.to_string()))?;
		Ok(format!("version = {}\n\n{body}", Self::VERSION))
	}

	/// The scene called `name`.
	pub fn scene(&self, name: &str) -> Option<&Scene> {
		self.scenes.iter().find(|scene| scene.name == name)
	}

	/// The canvas the scene called `scene` is drawn on, or the top layer for
	/// `None`: its own size, or the program's, at the program's frame rate.
	/// `None` for a scene that does not exist.
	pub fn canvas(&self, scene: Option<&str>) -> Option<Canvas> {
		match scene {
			None => Some(self.canvas),
			Some(name) => self.scene(name).map(|scene| scene.canvas(self.canvas)),
		}
	}

	/// The items of `layer`, or `None` for a scene that does not exist.
	pub fn items(&self, layer: &Layer) -> Option<&[Item]> {
		match &layer.scene {
			Some(scene) => self.scene(scene).map(|scene| scene.items.as_slice()),
			None => Some(&self.top),
		}
	}

	/// Run `edit` on the items of `layer`, or return `None` for a scene that
	/// does not exist.
	pub(crate) fn edit_items<T>(&mut self, layer: &Layer, edit: impl FnOnce(&mut Vec<Item>) -> T) -> Option<T> {
		match &layer.scene {
			None => Some(edit(&mut self.top)),
			Some(name) => {
				let scene = self.scenes.iter_mut().find(|scene| &scene.name == name)?;
				Some(edit(&mut scene.items))
			}
		}
	}

	/// Every layer: the scenes in order, then the top layer.
	pub fn layers(&self) -> impl Iterator<Item = (Layer, &[Item])> {
		let scenes = self
			.scenes
			.iter()
			.map(|scene| (Layer::scene(&scene.name), scene.items.as_slice()));
		scenes.chain([(Layer::top(), self.top.as_slice())])
	}

	/// The scene recordings hold, or `None` for the program.
	pub fn recorded(&self) -> Option<&str> {
		let recorded = self.scenes.iter().find(|scene| scene.recorded);
		recorded.map(|scene| scene.name.as_str())
	}

	/// The audio scene called `name`.
	pub fn audio_scene(&self, name: &str) -> Option<&AudioScene> {
		self.audio_scenes.iter().find(|scene| scene.name == name)
	}

	/// The audio scene the program broadcast carries. Only an invalid
	/// collection, such as one an edit is part way through, has none.
	pub fn program_audio(&self) -> Option<&str> {
		let program = self.audio_scenes.iter().find(|scene| scene.program);
		program.map(|scene| scene.name.as_str())
	}

	/// The audio scene recordings hold: the one picked for them, or the one
	/// on program.
	pub fn recorded_audio(&self) -> Option<&str> {
		let recorded = self.audio_scenes.iter().find(|scene| scene.recorded);
		recorded
			.map(|scene| scene.name.as_str())
			.or_else(|| self.program_audio())
	}

	/// The audio scene the scene called `scene` carries while published: the
	/// one it picks, or the one on program.
	pub fn published_audio(&self, scene: &str) -> Option<&str> {
		let picked = self.scene(scene).and_then(|scene| scene.audio.as_deref());
		picked.or_else(|| self.program_audio())
	}

	/// The sources whose sound the audio scene called `scene` plays: those it
	/// holds, and those of the audio scenes it holds, and so on, each once.
	pub fn heard(&self, scene: &str) -> BTreeSet<&str> {
		let mut heard = BTreeSet::new();
		for scene in self.reached_audio(scene) {
			heard.extend(scene.sounds.iter().map(String::as_str));
		}
		heard
	}

	/// Whether the audio scene called `scene` holds the one called `nested`,
	/// directly or through others.
	pub fn nests(&self, scene: &str, nested: &str) -> bool {
		let mut reached = self.reached_audio(scene).into_iter();
		reached.any(|scene| scene.scenes.contains(nested))
	}

	/// The audio scene called `scene` and every one it holds, directly or
	/// through others, each once.
	fn reached_audio(&self, scene: &str) -> Vec<&AudioScene> {
		let mut scenes = vec![scene];
		let mut seen = BTreeSet::new();
		let mut reached = Vec::new();
		while let Some(scene) = scenes.pop() {
			let Some(scene) = self.audio_scene(scene).filter(|scene| seen.insert(&scene.name)) else {
				continue;
			};
			scenes.extend(scene.scenes.iter().map(String::as_str));
			reached.push(scene);
		}
		reached
	}

	/// Whether the scene called `scene` can go on program: it is the
	/// program's size, and draws no program source, which it would draw into
	/// itself. Refused with the reason otherwise, or for a scene that does
	/// not exist.
	pub fn takes(&self, scene: &str) -> Result<(), Error> {
		let canvas = self
			.canvas(Some(scene))
			.ok_or_else(|| Error::Invalid(format!("there is no scene `{scene}`")))?;
		if canvas.size() != self.canvas.size() {
			return Err(Error::Invalid(format!(
				"`{scene}` is {}x{}, not the program's {}x{}, so it cannot go on program",
				canvas.width, canvas.height, self.canvas.width, self.canvas.height
			)));
		}
		if self.draws_program(scene) {
			return Err(Error::Invalid(format!(
				"`{scene}` draws the program, so it cannot go on program"
			)));
		}
		Ok(())
	}

	/// The source whose capture `name` shows: itself, or what it clones.
	pub fn capture<'a>(&'a self, name: &'a str) -> &'a str {
		match self.sources.get(name) {
			Some(Source::Clone { source }) => source,
			_ => name,
		}
	}

	/// What the source called `name` draws if it is a scene or program
	/// source, through a clone.
	pub fn nested(&self, name: &str) -> Option<Nested<'_>> {
		match self.sources.get(self.capture(name))? {
			Source::Scene { scene } => Some(Nested::Scene(scene)),
			Source::Program => Some(Nested::Program),
			_ => None,
		}
	}

	/// The visible items `layers` draw, then those of each scene they draw
	/// through a scene source, and so on, each scene once.
	pub fn drawn(&self, layers: impl IntoIterator<Item = Layer>) -> Vec<&Item> {
		let mut items: Vec<&Item> = layers
			.into_iter()
			.flat_map(|layer| self.items(&layer).unwrap_or_default())
			.filter(|item| item.visible)
			.collect();
		let mut nested = BTreeSet::new();
		let mut next = 0;
		while let Some(item) = items.get(next) {
			if let Some(Nested::Scene(scene)) = self.nested(&item.source)
				&& nested.insert(scene)
				&& let Some(scene) = self.scene(scene)
			{
				items.extend(scene.items.iter().filter(|item| item.visible));
			}
			next += 1;
		}
		items
	}

	/// Whether the scene called `scene` places a program source, itself or in
	/// a scene it draws, shown or hidden. It then cannot go on program, which
	/// it would draw into itself.
	pub fn draws_program(&self, scene: &str) -> bool {
		let items = self.reached(scene);
		items
			.iter()
			.any(|item| self.nested(&item.source) == Some(Nested::Program))
	}

	/// Every item of the scene called `scene` and of the scenes it draws,
	/// shown or hidden, each scene once.
	fn reached(&self, scene: &str) -> Vec<&Item> {
		let mut scenes = vec![scene];
		let mut seen = BTreeSet::new();
		let mut items = Vec::new();
		while let Some(scene) = scenes.pop() {
			let Some(scene) = self.scene(scene).filter(|scene| seen.insert(&scene.name)) else {
				continue;
			};
			for item in &scene.items {
				if let Some(Nested::Scene(scene)) = self.nested(&item.source) {
					scenes.push(scene);
				}
				items.push(item);
			}
		}
		items
	}

	/// Refuse a scene that draws itself, directly or through other scenes,
	/// shown or hidden, since it would never finish drawing.
	fn validate_nesting(&self) -> Result<(), Error> {
		// Walked with a stack of its own rather than the call stack, which a
		// long enough chain of scenes would overflow.
		let mut done = BTreeSet::new();
		for root in &self.scenes {
			if done.contains(root.name.as_str()) {
				continue;
			}
			// The scenes drawing the one walked, each with how many of its
			// items are walked.
			let mut path = vec![(root, 0)];
			while let Some(&mut (scene, ref mut walked)) = path.last_mut() {
				let Some(item) = scene.items.get(*walked) else {
					done.insert(scene.name.as_str());
					path.pop();
					continue;
				};
				*walked += 1;
				let Some(Nested::Scene(nested)) = self.nested(&item.source) else {
					continue;
				};
				let Some(nested) = self.scene(nested).filter(|nested| !done.contains(nested.name.as_str())) else {
					continue;
				};
				if let Some(at) = path.iter().position(|(on, _)| on.name == nested.name) {
					let through: Vec<String> = path[at + 1..].iter().map(|(on, _)| format!("`{}`", on.name)).collect();
					let mut err = format!("scene `{}` draws itself", nested.name);
					if !through.is_empty() {
						err = format!("{err} through {}", through.join(", "));
					}
					return Err(Error::Invalid(err));
				}
				path.push((nested, 0));
			}
		}

		for item in &self.top {
			let draws = match self.nested(&item.source) {
				Some(Nested::Program) => true,
				Some(Nested::Scene(scene)) => self.draws_program(scene),
				None => false,
			};
			if draws {
				return Err(Error::Invalid(format!(
					"the top layer places `{}`, which draws the program the top layer is drawn over",
					item.source
				)));
			}
		}
		Ok(())
	}

	/// The filters `item` draws through: its source's, then its own.
	pub fn filters<'a>(&'a self, item: &'a Item) -> impl Iterator<Item = &'a Filter> {
		let source = self.video.get(&item.source).map(|video| video.filters.as_slice());
		source.unwrap_or_default().iter().chain(&item.filters)
	}

	/// What `item` shows of its source's picture: its source's crop, then its
	/// own.
	pub fn crop(&self, item: &Item) -> Crop {
		let source = self.video.get(&item.source).map(|video| video.crop);
		source.unwrap_or_default().then(&item.crop)
	}

	/// Check what the types cannot: every canvas can be encoded, names are
	/// unique, a published scene's names its broadcast, one scene at most is
	/// recorded, every item, clone, audio, and video entry names a source that
	/// exists, a stinger names a media source, and every crop and filter's
	/// settings are in range, each filter naming a source of the kind it reads.
	pub fn validate(&self) -> Result<(), Error> {
		self.canvas.validate()?;

		let mut names = BTreeSet::new();
		for scene in &self.scenes {
			if scene.name.is_empty() {
				return Err(Error::Invalid("a scene has an empty name".into()));
			}
			if !names.insert(scene.name.as_str()) {
				return Err(Error::Invalid(format!("two scenes are named `{}`", scene.name)));
			}
			scene
				.canvas(self.canvas)
				.validate()
				.map_err(|err| Error::Invalid(format!("scene `{}`: {err}", scene.name)))?;
			if scene.published && (scene.name.contains('/') || moq_net::Path::new(&scene.name).is_err()) {
				return Err(Error::Invalid(format!(
					"scene `{}` is published, so its name is one segment of its broadcast's path: no `/`, and a `\\` only before another",
					scene.name
				)));
			}
		}
		let mut recorded = self.scenes.iter().filter(|scene| scene.recorded);
		if let (Some(first), Some(second)) = (recorded.next(), recorded.next()) {
			return Err(Error::Invalid(format!(
				"scenes `{}` and `{}` are both recorded, but recordings hold one",
				first.name, second.name
			)));
		}
		for (layer, items) in self.layers() {
			for item in items {
				let Some(source) = self.sources.get(&item.source) else {
					return Err(Error::Invalid(format!(
						"{layer} places `{}`, which is not a source",
						item.source
					)));
				};
				if matches!(source, Source::Lut { .. }) {
					return Err(Error::Invalid(format!(
						"{layer} places `{}`, a LUT, which only a filter can name",
						item.source
					)));
				}
				item.rect.validate(&layer, &item.source)?;
				let on = format!("`{}` in {layer}", item.source);
				item.crop
					.validate()
					.map_err(|err| Error::Invalid(format!("{on}: {err}")))?;
				self.validate_chain(&item.filters, &on)?;
				if layer.scene.is_none() {
					if !item.keyframes.is_empty() {
						return Err(Error::Invalid(format!(
							"{on} has keyframes, but the top layer never goes on to start them"
						)));
					}
					if let Some(Nested::Scene(scene)) = self.nested(&item.source)
						&& let Some(keyed) = self.reached(scene).into_iter().find(|item| !item.keyframes.is_empty())
					{
						return Err(Error::Invalid(format!(
							"{on} draws `{}` with keyframes, but the top layer never goes on to start them",
							keyed.source
						)));
					}
				}
				if !item.keyframes.is_sorted_by_key(|keyframe| keyframe.at) {
					return Err(Error::Invalid(format!("{on} has keyframes out of order")));
				}
				for keyframe in &item.keyframes {
					keyframe
						.validate()
						.map_err(|err| Error::Invalid(format!("{on}: {err}")))?;
				}
			}
		}

		if let Some(name) = self.sources.keys().find(|name| name.is_empty()) {
			return Err(Error::Invalid(format!("a source has an empty name: `{name}`")));
		}
		let unnamed = |source: &Source| source.path().is_some_and(|path| path.as_os_str().is_empty());
		if let Some((name, _)) = self.sources.iter().find(|(_, source)| unnamed(source)) {
			return Err(Error::Invalid(format!("`{name}` names no file")));
		}
		for (name, source) in &self.sources {
			source.validate(name)?;
			if let Source::Scene { scene } = source
				&& self.scene(scene).is_none()
			{
				return Err(Error::Invalid(format!(
					"`{name}` draws scene `{scene}`, which is not a scene"
				)));
			}
			let Source::Clone { source } = source else { continue };
			match self.sources.get(source) {
				Some(Source::Clone { .. }) => {
					return Err(Error::Invalid(format!(
						"`{name}` clones `{source}`, which is itself a clone"
					)));
				}
				Some(origin) if origin.has_video() => {}
				_ => {
					return Err(Error::Invalid(format!(
						"`{name}` clones `{source}`, which is not a source with a picture"
					)));
				}
			}
		}

		for (name, video) in &self.video {
			if !self.sources.get(name).is_some_and(Source::has_video) {
				return Err(Error::Invalid(format!(
					"video settings for `{name}`, which is not a source with a picture"
				)));
			}
			if video.background == Background::Remove && !self.sources[name].removes_background() {
				return Err(Error::Invalid(format!(
					"`{name}` cannot have its background removed: only a camera, display, window, image, media, \
					 broadcast, browser, or replay source can, and a clone shows its source's"
				)));
			}
			let on = format!("`{name}`");
			video
				.crop
				.validate()
				.map_err(|err| Error::Invalid(format!("{on}: {err}")))?;
			self.validate_chain(&video.filters, &on)?;
		}

		for name in &self.published {
			let Some(source) = self.sources.get(name) else {
				return Err(Error::Invalid(format!(
					"`{name}` is published, but there is no such source"
				)));
			};
			if !source.publishes() {
				return Err(Error::Invalid(format!(
					"`{name}` cannot be published independently: only a camera, display, window, microphone, \
					 system audio, color, image, media, or replay source has a track of its own"
				)));
			}
			if self
				.video
				.get(name)
				.is_some_and(|video| video.background == Background::Remove)
			{
				return Err(Error::Invalid(format!(
					"`{name}` cannot be published independently with its background removed, since its track \
					 carries no transparency and would show the background"
				)));
			}
		}

		self.validate_nesting()?;

		if let Some(name) = self.transition.stinger()
			&& !matches!(self.sources.get(name), Some(Source::Media { .. }))
		{
			return Err(Error::Invalid(format!(
				"the stinger plays `{name}`, which is not a media source"
			)));
		}

		self.validate_audio_scenes()?;
		for (name, audio) in &self.audio {
			if !self.sources.contains_key(name) {
				return Err(Error::Invalid(format!(
					"audio settings for `{name}`, which is not a source"
				)));
			}
			if !audio.gain.is_finite() || audio.gain < 0.0 {
				return Err(Error::Invalid(format!("`{name}` has gain {}", audio.gain)));
			}
			if !(-1.0..=1.0).contains(&audio.pan) {
				return Err(Error::Invalid(format!(
					"`{name}` has pan {}, outside -1 to 1",
					audio.pan
				)));
			}
			if audio.delay > moq_audio::bus::MAX_DELAY {
				return Err(Error::Invalid(format!(
					"`{name}` is held back {:?}, past the longest, {:?}",
					audio.delay,
					moq_audio::bus::MAX_DELAY
				)));
			}
		}

		Ok(())
	}

	/// Check the audio scenes: each named once, one on program and one at
	/// most recorded, each holding sources with sound and other audio scenes
	/// but never itself, and each scene's pick naming one.
	fn validate_audio_scenes(&self) -> Result<(), Error> {
		let mut names = BTreeSet::new();
		for scene in &self.audio_scenes {
			if scene.name.is_empty() {
				return Err(Error::Invalid("an audio scene has an empty name".into()));
			}
			if !names.insert(scene.name.as_str()) {
				return Err(Error::Invalid(format!("two audio scenes are named `{}`", scene.name)));
			}
		}
		let flagged = |flag: fn(&AudioScene) -> bool| -> Vec<String> {
			let scenes = self.audio_scenes.iter().filter(|scene| flag(scene));
			scenes.map(|scene| format!("`{}`", scene.name)).collect()
		};
		let program = flagged(|scene| scene.program);
		if program.len() != 1 {
			let found = match program.is_empty() {
				true => "none".to_string(),
				false => program.join(" and "),
			};
			return Err(Error::Invalid(format!("one audio scene is on program, not {found}")));
		}
		let recorded = flagged(|scene| scene.recorded);
		if recorded.len() > 1 {
			return Err(Error::Invalid(format!(
				"audio scenes {} are recorded, but recordings hold one",
				recorded.join(" and ")
			)));
		}
		for scene in &self.audio_scenes {
			let on = &scene.name;
			let silent = |sound: &&String| !self.sources.get(*sound).is_some_and(Source::has_audio);
			if let Some(sound) = scene.sounds.iter().find(silent) {
				return Err(Error::Invalid(format!(
					"audio scene `{on}` holds `{sound}`, which is not a source with sound"
				)));
			}
			if let Some(nested) = scene.scenes.iter().find(|nested| !names.contains(nested.as_str())) {
				return Err(Error::Invalid(format!(
					"audio scene `{on}` holds `{nested}`, which is not an audio scene"
				)));
			}
			if self.nests(on, on) {
				return Err(Error::Invalid(format!("audio scene `{on}` holds itself")));
			}
		}
		for scene in &self.scenes {
			if let Some(audio) = &scene.audio
				&& !names.contains(audio.as_str())
			{
				return Err(Error::Invalid(format!(
					"scene `{}` carries `{audio}`, which is not an audio scene",
					scene.name
				)));
			}
		}
		Ok(())
	}

	/// Check `filters` are in range and name sources of the kind they read.
	fn validate_chain(&self, filters: &[Filter], on: &str) -> Result<(), Error> {
		validate_filters(filters, on)?;
		for filter in filters {
			let (name, ok, wanted) = match filter {
				Filter::Lut { source, .. } => (
					source,
					matches!(self.sources.get(source), Some(Source::Lut { .. })),
					"a LUT",
				),
				Filter::Mask { source, .. } => (
					source,
					self.sources
						.get(source)
						.is_some_and(|source| source.has_video() && source.opens()),
					"a source with a picture of its own",
				),
				_ => continue,
			};
			if !ok {
				return Err(Error::Invalid(format!(
					"{on}: a filter names `{name}`, which is not {wanted}"
				)));
			}
		}
		Ok(())
	}
}

/// Parse a TOML file of this build's [`Collection::VERSION`], without its
/// `version` key: the scene file, and the history and clipboard formats that
/// carry its sources and items.
pub(crate) fn versioned(text: &str) -> Result<toml::Table, Error> {
	let mut table: toml::Table = toml::from_str(text).map_err(|err| Error::Format(err.to_string()))?;
	let version = match table.remove("version") {
		Some(toml::Value::Integer(version)) => version,
		Some(other) => return Err(Error::Format(format!("`version` must be an integer, got {other}"))),
		None => return Err(Error::Format("missing `version`".into())),
	};
	if version != i64::from(Collection::VERSION) {
		return Err(Error::Version {
			found: version,
			supported: Collection::VERSION,
		});
	}
	Ok(table)
}

impl Default for Collection {
	fn default() -> Self {
		Self::new()
	}
}

/// The output picture: its size and how often it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Canvas {
	/// Width in pixels. Even, since the encoder subsamples chroma.
	pub width: u32,
	/// Height in pixels. Even, since the encoder subsamples chroma.
	pub height: u32,
	/// Pictures drawn and encoded per second.
	pub framerate: u32,
}

impl Canvas {
	/// The canvas size.
	pub fn size(&self) -> moq_video::Size {
		moq_video::Size::new(self.width, self.height)
	}

	/// How often a picture is drawn.
	pub fn framerate(&self) -> moq_video::FrameRate {
		moq_video::FrameRate::new(self.framerate, 1).expect("validated canvas framerate")
	}

	pub(crate) fn validate(&self) -> Result<(), Error> {
		if self.width == 0 || self.height == 0 || !self.width.is_multiple_of(2) || !self.height.is_multiple_of(2) {
			return Err(Error::Invalid(format!(
				"canvas {}x{} must be non-zero and even",
				self.width, self.height
			)));
		}
		moq_video::FrameRate::new(self.framerate, 1)
			.map_err(|err| Error::Invalid(format!("canvas framerate {}: {err}", self.framerate)))?;
		Ok(())
	}
}

impl Default for Canvas {
	fn default() -> Self {
		Self {
			width: 1920,
			height: 1080,
			framerate: 30,
		}
	}
}

/// A scene's own size, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Size {
	/// Width in pixels. Even, since the encoder subsamples chroma.
	pub width: u32,
	/// Height in pixels. Even, since the encoder subsamples chroma.
	pub height: u32,
}

/// How the program changes when the preview is taken to it, short of a cut.
///
/// A fade runs at an even pace; the slide, wipe, and move ease in and out, so
/// motion starts and lands gently. One that takes no time is a cut.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Transition {
	/// The incoming scene fades in over the outgoing one.
	Fade {
		/// How long it takes.
		#[serde(with = "humantime_serde")]
		duration: Duration,
	},
	/// The incoming scene pushes the outgoing one off the canvas, both moving
	/// in `direction`.
	Slide {
		/// How long it takes.
		#[serde(with = "humantime_serde")]
		duration: Duration,
		/// Which way the scenes move.
		direction: Direction,
	},
	/// An edge sweeps across the canvas in `direction`, uncovering the
	/// incoming scene behind it.
	Wipe {
		/// How long it takes.
		#[serde(with = "humantime_serde")]
		duration: Duration,
		/// Which way the edge moves.
		direction: Direction,
	},
	/// Each item showing a source both scenes place glides from where the
	/// outgoing scene has it to where the incoming one does, resizing and
	/// recropping on the way; the other items fade out and in.
	Move {
		/// How long it takes.
		#[serde(with = "humantime_serde")]
		duration: Duration,
	},
	/// A media source's clip plays over the program from its start, and the
	/// program switches scenes under it at `cut` into the clip, while it
	/// covers the canvas. It lasts as long as the clip's first play, filling
	/// the canvas, cropped to its shape, through the source's own crop and
	/// filters.
	Stinger {
		/// The [`Source::Media`] whose clip plays.
		source: String,
		/// How far into the clip the scenes switch.
		#[serde(with = "humantime_serde")]
		cut: Duration,
	},
}

impl Transition {
	/// How long it takes, or `None` for a stinger, which lasts as long as its
	/// clip.
	pub fn duration(&self) -> Option<Duration> {
		match *self {
			Self::Fade { duration }
			| Self::Slide { duration, .. }
			| Self::Wipe { duration, .. }
			| Self::Move { duration } => Some(duration),
			Self::Stinger { .. } => None,
		}
	}

	/// How long it takes, to change, or `None` for a stinger.
	pub fn duration_mut(&mut self) -> Option<&mut Duration> {
		match self {
			Self::Fade { duration }
			| Self::Slide { duration, .. }
			| Self::Wipe { duration, .. }
			| Self::Move { duration } => Some(duration),
			Self::Stinger { .. } => None,
		}
	}

	/// The media source a stinger plays.
	pub fn stinger(&self) -> Option<&str> {
		match self {
			Self::Stinger { source, .. } => Some(source),
			_ => None,
		}
	}

	/// How far along it looks once `linear` of its time has passed, both from
	/// 0 to 1: as is for a fade, and eased in and out otherwise.
	pub(crate) fn progress(&self, linear: f32) -> f32 {
		match self {
			Self::Fade { .. } => Ease::Linear.apply(linear),
			_ => Ease::InOut.apply(linear),
		}
	}
}

/// How a change speeds up and slows down over its time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
	/// At an even pace.
	Linear,
	/// Starting slowly (cubic).
	In,
	/// Landing slowly (cubic).
	Out,
	/// Starting and landing slowly (cubic).
	#[default]
	InOut,
}

impl Ease {
	/// Every curve.
	pub const ALL: [Self; 4] = [Self::Linear, Self::In, Self::Out, Self::InOut];

	/// How far along the change looks once `linear` of its time has passed,
	/// both from 0 to 1.
	pub(crate) fn apply(self, linear: f32) -> f32 {
		let linear = linear.clamp(0.0, 1.0);
		match self {
			Self::Linear => linear,
			Self::In => linear.powi(3),
			Self::Out => 1.0 - (1.0 - linear).powi(3),
			Self::InOut if linear < 0.5 => 4.0 * linear.powi(3),
			Self::InOut => 1.0 - (2.0 - 2.0 * linear).powi(3) / 2.0,
		}
	}

	fn is_default(&self) -> bool {
		*self == Self::default()
	}
}

impl Default for Transition {
	fn default() -> Self {
		Self::Fade {
			duration: Duration::from_millis(300),
		}
	}
}

/// Which way a [`Transition`] moves across the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
	/// Toward the left edge.
	Left,
	/// Toward the right edge.
	Right,
	/// Toward the top edge.
	Up,
	/// Toward the bottom edge.
	Down,
}

impl Direction {
	/// Every direction.
	pub const ALL: [Self; 4] = [Self::Left, Self::Right, Self::Up, Self::Down];
}

/// One arrangement of sources on a canvas of its own size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
	/// Unique among the collection's scenes.
	pub name: String,
	/// Its size, or `None` for the program's. Only a scene of the program's
	/// size goes on program.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub size: Option<Size>,
	/// Whether it is always live as a broadcast of its own, beneath the
	/// program's and named after it, whatever is on program.
	#[serde(default, skip_serializing_if = "is_false")]
	pub published: bool,
	/// Whether recordings hold it rather than the program. One scene at most
	/// is recorded.
	#[serde(default, skip_serializing_if = "is_false")]
	pub recorded: bool,
	/// The audio scene its broadcast carries while published, or `None` for
	/// the one on program.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub audio: Option<String>,
	/// What the scene draws, back to front.
	#[serde(default)]
	pub items: Vec<Item>,
}

impl Scene {
	/// An empty scene of the program's size, neither published nor recorded,
	/// carrying the audio scene on program.
	pub fn new(name: impl Into<String>) -> Self {
		Self {
			name: name.into(),
			size: None,
			published: false,
			recorded: false,
			audio: None,
			items: Vec::new(),
		}
	}

	/// The canvas it is drawn on: its own size, or `program`'s, at
	/// `program`'s frame rate.
	pub fn canvas(&self, program: Canvas) -> Canvas {
		match self.size {
			Some(Size { width, height }) => Canvas {
				width,
				height,
				framerate: program.framerate,
			},
			None => program,
		}
	}
}

/// Where a list of items lives: a scene, or the top layer over the program.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
	/// The scene, or `None` for the top layer.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub scene: Option<String>,
}

impl Layer {
	/// The scene called `name`.
	pub fn scene(name: impl Into<String>) -> Self {
		Self {
			scene: Some(name.into()),
		}
	}

	/// The top layer.
	pub fn top() -> Self {
		Self::default()
	}
}

impl fmt::Display for Layer {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match &self.scene {
			Some(name) => write!(f, "scene `{name}`"),
			None => write!(f, "the top layer"),
		}
	}
}

/// What a [`Source::Scene`] or [`Source::Program`] draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Nested<'a> {
	/// The scene by this name.
	Scene(&'a str),
	/// The program.
	Program,
}

/// A source placed in a scene or the top layer.
///
/// Every item naming the same source shows the same capture, with the
/// source's filters: the items are linked. A [`Source::Clone`] shows another
/// source's capture with filters of its own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
	/// The [`Collection::sources`] entry to draw.
	pub source: String,
	/// Where on the canvas, stretched to fill it.
	pub rect: Rect,
	/// Whether the item is drawn. A hidden item keeps its place.
	#[serde(default = "yes")]
	pub visible: bool,
	/// Trimmed from what its source's crop leaves, before every filter.
	#[serde(default, skip_serializing_if = "Crop::is_none")]
	pub crop: Crop,
	/// This item's own filters, applied after its source's.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub filters: Vec<Filter>,
	/// How the filtered picture mixes with what is under it.
	#[serde(default, skip_serializing_if = "Blend::is_normal")]
	pub blend: Blend,
	/// How the item moves away from and back to where it rests, from when its
	/// scene goes on, in order of time. It plays once: before the first
	/// keyframe the item holds that one, and after the last it holds the last.
	/// A top-layer item has none, nor does a scene it draws, since the top
	/// layer never goes on.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub keyframes: Vec<Keyframe>,
}

impl Item {
	/// `source` shown, uncropped, unfiltered, and still, at `rect`.
	pub fn new(source: impl Into<String>, rect: Rect) -> Self {
		Self {
			source: source.into(),
			rect,
			visible: true,
			crop: Crop::default(),
			filters: Vec::new(),
			blend: Blend::Normal,
			keyframes: Vec::new(),
		}
	}

	/// The item as it is drawn `since` its scene went on: moved, resized,
	/// cropped, and filtered by its keyframes, which the result has none of.
	pub fn posed(&self, since: Duration) -> Cow<'_, Self> {
		let Some(offset) = self.offset(since) else {
			return Cow::Borrowed(self);
		};
		let mut filters = self.filters.clone();
		filters.extend(offset.filters);
		Cow::Owned(Self {
			source: self.source.clone(),
			rect: Rect {
				x: self.rect.x + offset.x,
				y: self.rect.y + offset.y,
				// A size shrunk past nothing draws nothing.
				width: (self.rect.width + offset.width).max(0.0),
				height: (self.rect.height + offset.height).max(0.0),
			},
			visible: self.visible,
			crop: self.crop.then(&offset.crop),
			filters,
			blend: self.blend,
			keyframes: Vec::new(),
		})
	}

	/// Where `since` falls among the keyframes, so two times that settle the
	/// same pose the item the same.
	pub(crate) fn settled(&self, since: Duration) -> Settled {
		match (self.keyframes.first(), self.keyframes.last()) {
			(Some(first), _) if since < first.at => Settled::Before,
			(Some(_), Some(last)) if since < last.at => Settled::At(since),
			_ => Settled::After,
		}
	}

	/// How far the keyframes take the item from where it rests `since` its
	/// scene went on, or `None` if it has none.
	fn offset(&self, since: Duration) -> Option<Keyframe> {
		let next = self.keyframes.iter().position(|keyframe| keyframe.at > since);
		let (from, to) = match next {
			None => return self.keyframes.last().cloned(),
			Some(0) => return self.keyframes.first().cloned(),
			Some(next) => (&self.keyframes[next - 1], &self.keyframes[next]),
		};
		// `to` is later than `since`, and `from` no later, so the span is not empty.
		let linear = (since - from.at).as_secs_f32() / (to.at - from.at).as_secs_f32();
		Some(from.toward(to, to.ease.apply(linear)))
	}
}

/// Where a moment falls among an item's keyframes: holding the first, between
/// two at that moment, or holding the last. Keyframes at the same moment jump,
/// so the first keyframe's moment is not the same as before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Settled {
	Before,
	At(Duration),
	After,
}

/// Where an item is at a moment after its scene goes on, as an offset from
/// where it rests, so moving the item moves its whole animation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keyframe {
	/// How long after the item's scene goes on.
	#[serde(with = "humantime_serde")]
	pub at: Duration,
	/// How the item eases into this keyframe from the one before.
	#[serde(default, skip_serializing_if = "Ease::is_default")]
	pub ease: Ease,
	/// Canvas pixels added to the item's left edge.
	#[serde(default, skip_serializing_if = "is_zero")]
	pub x: f32,
	/// Canvas pixels added to the item's top edge.
	#[serde(default, skip_serializing_if = "is_zero")]
	pub y: f32,
	/// Canvas pixels added to the item's width.
	#[serde(default, skip_serializing_if = "is_zero")]
	pub width: f32,
	/// Canvas pixels added to the item's height.
	#[serde(default, skip_serializing_if = "is_zero")]
	pub height: f32,
	/// Trimmed from what the item's crop leaves, as a push-in does.
	#[serde(default, skip_serializing_if = "Crop::is_none")]
	pub crop: Crop,
	/// Applied after the item's own. Each one's settings mix into the next
	/// keyframe's when the two lists pair up by kind, in order, and otherwise
	/// change halfway. None reads another source.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub filters: Vec<Filter>,
}

impl Keyframe {
	/// The item where it rests, `at` after its scene goes on.
	pub fn new(at: Duration) -> Self {
		Self {
			at,
			ease: Ease::default(),
			x: 0.0,
			y: 0.0,
			width: 0.0,
			height: 0.0,
			crop: Crop::default(),
			filters: Vec::new(),
		}
	}

	/// `progress` of the way from this keyframe's offset to `to`'s.
	fn toward(&self, to: &Self, progress: f32) -> Self {
		let mix = |from: f32, to: f32| mix(from, to, progress);
		let paired = self.filters.len() == to.filters.len();
		let pairs = self.filters.iter().zip(&to.filters);
		let filters = paired.then(|| pairs.map(|(from, to)| from.toward(to, progress)).collect());
		Self {
			at: to.at,
			ease: to.ease,
			x: mix(self.x, to.x),
			y: mix(self.y, to.y),
			width: mix(self.width, to.width),
			height: mix(self.height, to.height),
			crop: self.crop.toward(&to.crop, progress),
			filters: filters
				.flatten()
				.unwrap_or_else(|| if progress < 0.5 { &self.filters } else { &to.filters }.clone()),
		}
	}

	fn validate(&self) -> Result<(), String> {
		let offsets = [self.x, self.y, self.width, self.height];
		if !offsets.iter().all(|offset| offset.is_finite()) {
			return Err(format!("a keyframe at {:?} offsets by {offsets:?}", self.at));
		}
		self.crop.validate()?;
		if let Some(source) = self.filters.iter().find_map(Filter::source) {
			return Err(format!(
				"a keyframe at {:?} has a filter reading `{source}`; a keyframe's filters read no other source",
				self.at
			));
		}
		self.filters.iter().try_for_each(Filter::validate)
	}
}

fn is_zero(value: &f32) -> bool {
	*value == 0.0
}

fn yes() -> bool {
	true
}

fn is_false(value: &bool) -> bool {
	!value
}

fn is_true(value: &bool) -> bool {
	*value
}

/// What a source or item is drawn through, in order.
///
/// Every filter is a pure function of the picture, so every host that composes
/// a scene with moq-scene draws it the same way, and it travels in a layout as
/// its name and settings. The CSS ones mean what the CSS Filter Effects
/// function of the same name does, in the gamma-encoded sRGB the canvas holds,
/// with lengths in canvas pixels. A filter naming another source uses that
/// source's picture as it arrives, without its filters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Filter {
	/// CSS `opacity()`: 0 is transparent and 1 leaves the picture as it is.
	Opacity {
		/// From 0 to 1.
		amount: f32,
	},
	/// CSS `brightness()`: each color channel times `amount`.
	Brightness {
		/// 0 or more: 0 is black and 1 leaves the picture as it is.
		amount: f32,
	},
	/// CSS `contrast()`: each color channel pushed away from middle gray.
	Contrast {
		/// 0 or more: 0 is gray and 1 leaves the picture as it is.
		amount: f32,
	},
	/// CSS `saturate()`.
	Saturate {
		/// 0 or more: 0 is gray, 1 leaves the picture as it is, and more
		/// oversaturates.
		amount: f32,
	},
	/// CSS `grayscale()`.
	Grayscale {
		/// From 0 to 1, where 1 is fully gray.
		amount: f32,
	},
	/// CSS `sepia()`.
	Sepia {
		/// From 0 to 1, where 1 is fully sepia.
		amount: f32,
	},
	/// CSS `invert()`.
	Invert {
		/// From 0 to 1, where 1 is a negative.
		amount: f32,
	},
	/// CSS `hue-rotate()`.
	HueRotate {
		/// The angle around the color wheel.
		degrees: f32,
	},
	/// CSS `blur()`: a Gaussian blur whose edges fade into transparency.
	Blur {
		/// The standard deviation, in canvas pixels; 0 or more.
		radius: f32,
	},
	/// CSS `drop-shadow()`: the picture's shape, offset, blurred, and filled
	/// with `color`, drawn under it.
	DropShadow {
		/// Offset right, in canvas pixels.
		x: f32,
		/// Offset down, in canvas pixels.
		y: f32,
		/// The blur's standard deviation, in canvas pixels; 0 or more.
		blur: f32,
		/// The shadow's color.
		color: Rgba,
	},
	/// Make what is close to `color` transparent, as a green screen is keyed.
	///
	/// Closeness is the distance between the Rec. 709 chroma (Cb, Cr) of a
	/// pixel and of `color`, so a shadow on the screen keys like its lit part.
	/// Within `similarity` a pixel is transparent, and over the next
	/// `smoothness` it fades in. Within `similarity + spill`, the key color
	/// bleeding onto the subject is desaturated away.
	ChromaKey {
		/// The screen's color.
		color: Rgba,
		/// From 0 to 1.
		similarity: f32,
		/// From 0 to 1.
		smoothness: f32,
		/// From 0 to 1.
		spill: f32,
	},
	/// Make what is too dark or too bright transparent, by Rec. 709 luma.
	///
	/// Luma from `min` to `max` stays opaque, and over `smoothness` past
	/// either end fades out.
	LumaKey {
		/// From 0 to 1.
		min: f32,
		/// From `min` to 1.
		max: f32,
		/// From 0 to 1.
		smoothness: f32,
	},
	/// Look each color up in a [`Source::Lut`]'s table, as a color grade does.
	Lut {
		/// The [`Source::Lut`].
		source: String,
		/// From 0 (the picture as it is) to 1 (fully graded).
		amount: f32,
	},
	/// Keep the picture only where another source's picture, stretched over
	/// the item, is opaque, as CSS `mask-image` does.
	Mask {
		/// A source with a picture of its own: not a clone.
		source: String,
		/// What of the mask's picture counts.
		#[serde(default)]
		mode: MaskMode,
	},
}

/// What of a mask's picture keeps the picture under it, as CSS `mask-mode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskMode {
	/// Its opacity.
	#[default]
	Alpha,
	/// Its Rec. 709 luma times its opacity, so white keeps and black hides.
	Luminance,
}

impl Filter {
	/// The source whose picture or table this filter reads, if any.
	pub fn source(&self) -> Option<&str> {
		match self {
			Self::Lut { source, .. } | Self::Mask { source, .. } => Some(source),
			_ => None,
		}
	}

	/// `progress` of the way from this filter's settings to `other`'s, or
	/// `None` if they are of different kinds or read different sources or
	/// mask modes. A mix of valid settings is valid.
	pub(crate) fn toward(&self, other: &Self, progress: f32) -> Option<Self> {
		let mix = |from: f32, to: f32| mix(from, to, progress);
		let color = |from: Rgba, to: Rgba| {
			Rgba(std::array::from_fn(|channel| {
				mix(f32::from(from.0[channel]), f32::from(to.0[channel])).round() as u8
			}))
		};
		Some(match (self, other) {
			(Self::Opacity { amount: a }, Self::Opacity { amount: b }) => Self::Opacity { amount: mix(*a, *b) },
			(Self::Brightness { amount: a }, Self::Brightness { amount: b }) => {
				Self::Brightness { amount: mix(*a, *b) }
			}
			(Self::Contrast { amount: a }, Self::Contrast { amount: b }) => Self::Contrast { amount: mix(*a, *b) },
			(Self::Saturate { amount: a }, Self::Saturate { amount: b }) => Self::Saturate { amount: mix(*a, *b) },
			(Self::Grayscale { amount: a }, Self::Grayscale { amount: b }) => Self::Grayscale { amount: mix(*a, *b) },
			(Self::Sepia { amount: a }, Self::Sepia { amount: b }) => Self::Sepia { amount: mix(*a, *b) },
			(Self::Invert { amount: a }, Self::Invert { amount: b }) => Self::Invert { amount: mix(*a, *b) },
			(Self::HueRotate { degrees: a }, Self::HueRotate { degrees: b }) => {
				Self::HueRotate { degrees: mix(*a, *b) }
			}
			(Self::Blur { radius: a }, Self::Blur { radius: b }) => Self::Blur { radius: mix(*a, *b) },
			(
				Self::DropShadow { x, y, blur, color: c },
				Self::DropShadow {
					x: x2,
					y: y2,
					blur: blur2,
					color: c2,
				},
			) => Self::DropShadow {
				x: mix(*x, *x2),
				y: mix(*y, *y2),
				blur: mix(*blur, *blur2),
				color: color(*c, *c2),
			},
			(
				Self::ChromaKey {
					color: c,
					similarity,
					smoothness,
					spill,
				},
				Self::ChromaKey {
					color: c2,
					similarity: similarity2,
					smoothness: smoothness2,
					spill: spill2,
				},
			) => Self::ChromaKey {
				color: color(*c, *c2),
				similarity: mix(*similarity, *similarity2),
				smoothness: mix(*smoothness, *smoothness2),
				spill: mix(*spill, *spill2),
			},
			(
				Self::LumaKey { min, max, smoothness },
				Self::LumaKey {
					min: min2,
					max: max2,
					smoothness: smoothness2,
				},
			) => Self::LumaKey {
				min: mix(*min, *min2),
				max: mix(*max, *max2),
				smoothness: mix(*smoothness, *smoothness2),
			},
			(Self::Lut { source, amount: a }, Self::Lut { source: to, amount: b }) if source == to => Self::Lut {
				source: source.clone(),
				amount: mix(*a, *b),
			},
			(Self::Mask { .. }, Self::Mask { .. }) if self == other => self.clone(),
			_ => return None,
		})
	}

	/// The source this filter reads, to rename it.
	pub(crate) fn source_mut(&mut self) -> Option<&mut String> {
		match self {
			Self::Lut { source, .. } | Self::Mask { source, .. } => Some(source),
			_ => None,
		}
	}

	fn validate(&self) -> Result<(), String> {
		fn check(ok: bool, err: impl FnOnce() -> String) -> Result<(), String> {
			if ok { Ok(()) } else { Err(err()) }
		}
		let unit = |name: &str, value: f32| {
			check((0.0..=1.0).contains(&value), || {
				format!("{name} {value} is not between 0 and 1")
			})
		};
		let positive = |name: &str, value: f32| {
			check(value.is_finite() && value >= 0.0, || {
				format!("{name} {value} is not 0 or more")
			})
		};
		let finite = |name: &str, value: f32| check(value.is_finite(), || format!("{name} {value} is not a number"));

		match *self {
			Self::Opacity { amount } => unit("opacity", amount),
			Self::Brightness { amount } => positive("brightness", amount),
			Self::Contrast { amount } => positive("contrast", amount),
			Self::Saturate { amount } => positive("saturation", amount),
			Self::Grayscale { amount } => unit("grayscale", amount),
			Self::Sepia { amount } => unit("sepia", amount),
			Self::Invert { amount } => unit("invert", amount),
			Self::HueRotate { degrees } => finite("hue rotation", degrees),
			Self::Blur { radius } => positive("blur", radius),
			Self::DropShadow { x, y, blur, .. } => {
				finite("shadow x", x)?;
				finite("shadow y", y)?;
				positive("shadow blur", blur)
			}
			Self::ChromaKey {
				similarity,
				smoothness,
				spill,
				..
			} => {
				unit("similarity", similarity)?;
				unit("smoothness", smoothness)?;
				unit("spill", spill)
			}
			Self::LumaKey { min, max, smoothness } => {
				unit("luma min", min)?;
				unit("luma max", max)?;
				unit("smoothness", smoothness)?;
				check(min <= max, || format!("luma min {min} is over max {max}"))
			}
			Self::Lut { amount, .. } => unit("LUT amount", amount),
			Self::Mask { .. } => Ok(()),
		}
	}
}

/// How an item's picture mixes with what is under it, as CSS
/// `mix-blend-mode`, by the formulas of the Compositing and Blending spec.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[expect(missing_docs, reason = "each is the CSS blend mode of the same name")]
pub enum Blend {
	/// Drawn over, as without a blend mode.
	#[default]
	Normal,
	Multiply,
	Screen,
	Overlay,
	Darken,
	Lighten,
	ColorDodge,
	ColorBurn,
	HardLight,
	SoftLight,
	Difference,
	Exclusion,
	Hue,
	Saturation,
	Color,
	Luminosity,
}

impl Blend {
	/// Every mode, in the CSS spec's order.
	pub const ALL: [Self; 16] = [
		Self::Normal,
		Self::Multiply,
		Self::Screen,
		Self::Overlay,
		Self::Darken,
		Self::Lighten,
		Self::ColorDodge,
		Self::ColorBurn,
		Self::HardLight,
		Self::SoftLight,
		Self::Difference,
		Self::Exclusion,
		Self::Hue,
		Self::Saturation,
		Self::Color,
		Self::Luminosity,
	];

	fn is_normal(&self) -> bool {
		*self == Self::Normal
	}
}

/// The fraction of a picture trimmed from each edge, the rest stretched to
/// fill the item, as CSS `object-view-box: inset()` does.
///
/// Fractions rather than pixels, so a crop keeps its framing when the
/// picture's resolution changes, such as a viewer picking another rendition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crop {
	/// From the left edge.
	#[serde(default)]
	pub left: f32,
	/// From the top edge.
	#[serde(default)]
	pub top: f32,
	/// From the right edge.
	#[serde(default)]
	pub right: f32,
	/// From the bottom edge.
	#[serde(default)]
	pub bottom: f32,
}

impl Crop {
	/// Whether the crop trims nothing.
	pub fn is_none(&self) -> bool {
		*self == Self::default()
	}

	/// `inner` applied to the picture this crop leaves, as one crop.
	pub fn then(&self, inner: &Self) -> Self {
		let width = 1.0 - self.left - self.right;
		let height = 1.0 - self.top - self.bottom;
		Self {
			left: self.left + inner.left * width,
			top: self.top + inner.top * height,
			right: self.right + inner.right * width,
			bottom: self.bottom + inner.bottom * height,
		}
	}

	/// `progress` of the way from this crop to `other`, which a mix of two
	/// valid crops is.
	pub(crate) fn toward(&self, other: &Self, progress: f32) -> Self {
		let mix = |from: f32, to: f32| mix(from, to, progress);
		Self {
			left: mix(self.left, other.left),
			top: mix(self.top, other.top),
			right: mix(self.right, other.right),
			bottom: mix(self.bottom, other.bottom),
		}
	}

	fn validate(&self) -> Result<(), String> {
		let edges = [self.left, self.top, self.right, self.bottom];
		if edges.iter().any(|edge| !(0.0..1.0).contains(edge))
			|| self.left + self.right >= 1.0
			|| self.top + self.bottom >= 1.0
		{
			return Err(format!("{self:?} leaves nothing of the picture"));
		}
		Ok(())
	}
}

/// `progress` of the way from `from` to `to`, finite whenever both are, since
/// it never leaves the range between them.
pub(crate) fn mix(from: f32, to: f32, progress: f32) -> f32 {
	// In f64, where neither product can overflow, and each term's weight
	// rounds no larger than it is.
	let progress = f64::from(progress);
	(f64::from(from) * (1.0 - progress) + f64::from(to) * progress) as f32
}

pub(crate) fn validate_filters(filters: &[Filter], on: &str) -> Result<(), Error> {
	for filter in filters {
		filter
			.validate()
			.map_err(|err| Error::Invalid(format!("{on}: {err}")))?;
	}
	Ok(())
}

/// How one source's picture is processed wherever it is placed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Video {
	/// What happens behind the person in the picture, before the crop and
	/// every filter.
	#[serde(default, skip_serializing_if = "Background::is_kept")]
	pub background: Background,
	/// Trimmed before every filter and each item's own crop.
	#[serde(default, skip_serializing_if = "Crop::is_none")]
	pub crop: Crop,
	/// Applied in order, before each item's own.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub filters: Vec<Filter>,
}

/// What happens to the background behind a person in a source's picture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Background {
	/// Shown as captured.
	#[default]
	Keep,
	/// Made transparent, leaving the person a segmentation model finds in each
	/// picture. Each picture draws once its own cut-out is ready, a frame or
	/// more late, skipping pictures while the model is behind. The
	/// source's own track keeps the background, since its encoding carries no
	/// transparency.
	Remove,
}

impl Background {
	fn is_kept(&self) -> bool {
		*self == Self::Keep
	}
}

/// A rectangle in canvas pixels, from the top-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
	/// Left edge.
	pub x: f32,
	/// Top edge.
	pub y: f32,
	/// Width.
	pub width: f32,
	/// Height.
	pub height: f32,
}

impl Rect {
	/// The whole of `canvas`.
	pub fn fill(canvas: &Canvas) -> Self {
		Self {
			x: 0.0,
			y: 0.0,
			width: canvas.width as f32,
			height: canvas.height as f32,
		}
	}

	fn validate(&self, layer: &Layer, source: &str) -> Result<(), Error> {
		let finite = [self.x, self.y, self.width, self.height].iter().all(|v| v.is_finite());
		if !finite || self.width < 0.0 || self.height < 0.0 {
			return Err(Error::Invalid(format!(
				"{layer} places `{source}` at {self:?}, which is not a rectangle"
			)));
		}
		Ok(())
	}
}

/// Where a source's picture or sound comes from.
///
/// Device ids are the ones the matching `moq_video::capture` and
/// `moq_audio::capture` listings report; `None` means the system default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
	/// A camera.
	Camera {
		/// The camera's id, or the default camera.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		id: Option<String>,
	},
	/// A whole display.
	Display {
		/// The display's id, or the main display.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		id: Option<String>,
	},
	/// One window.
	Window {
		/// The window's id.
		id: String,
	},
	/// A microphone or other audio input.
	Microphone {
		/// The input's id, or the default input.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		id: Option<String>,
	},
	/// Everything the machine plays, where the platform can capture it.
	SystemAudio,
	/// A PNG or JPEG still.
	Image {
		/// The file, relative to the scene file's directory unless absolute.
		path: PathBuf,
	},
	/// A video or audio file, such as an intro clip or a music bed, played on
	/// the program clock from its start, and again whenever
	/// [`Feed::play`](crate::source::Feed::play) asks.
	Media {
		/// The file, relative to the scene file's directory unless absolute.
		path: PathBuf,
		/// Whether it starts over at its end, rather than holding its last
		/// picture and going silent.
		#[serde(default, rename = "loop", skip_serializing_if = "std::ops::Not::not")]
		looping: bool,
		/// Whether it plays from its start each time it goes on program.
		#[serde(default, skip_serializing_if = "std::ops::Not::not")]
		restart: bool,
	},
	/// A solid color.
	Color {
		/// The color, as `#rrggbb` or `#rrggbbaa`.
		color: Rgba,
	},
	/// A broadcast subscribed from the relay the studio is connected to.
	Broadcast {
		/// The broadcast path on that relay.
		path: String,
	},
	/// A web page, such as an overlay, alert box, or chat widget, with its
	/// sound. Desktop only: viewers cannot render one, so it reaches them
	/// only as pixels.
	Browser {
		/// The page's address.
		url: String,
		/// The page's viewport width in pixels, independent of where it is
		/// placed.
		width: u32,
		/// The page's viewport height in pixels.
		height: u32,
	},
	/// A graphic written as HTML and CSS without scripts, such as a title or
	/// lower third. Its CSS animations run on the program clock, so they land
	/// on the same frame in the program and everything made from it. It loads
	/// only images and fonts from `data:` URLs and files under the scene
	/// file's directory, and HTML, CSS, or an asset outside what overlays draw
	/// fails the source, naming it.
	Overlay {
		/// The document, its CSS in `<style>` elements and `style` attributes.
		html: String,
		/// The overlay's viewport width in pixels, independent of where it is
		/// placed.
		width: u32,
		/// The overlay's viewport height in pixels.
		height: u32,
		/// Whether its CSS timeline starts over each time a scene showing it
		/// goes on program, or into preview while program does not show it,
		/// as keyframes do, rather than running on from when it opened, as a
		/// clock should.
		#[serde(default = "yes", skip_serializing_if = "is_true")]
		restart: bool,
	},
	/// Another source's picture, captured once, with [`Collection::video`]
	/// filters of its own. Its sound is not mixed a second time.
	Clone {
		/// The source cloned: one with a picture, and not itself a clone.
		source: String,
	},
	/// A 3D color lookup table for [`Filter::Lut`], from an Adobe or Resolve
	/// `.cube` file. It has no picture or sound of its own.
	Lut {
		/// The file, relative to the scene file's directory unless absolute.
		path: PathBuf,
	},
	/// Where instant replays play: the recent past of the program or another
	/// source, on demand through [`Feed::replay`](crate::source::Feed::replay).
	/// Transparent while nothing plays.
	Replay,
	/// Another scene, drawn whole at its own size, as a picture. Its
	/// items' keyframes play from when the scene drawing it goes on, as that
	/// scene's own items' do, so the top layer cannot draw one with keyframes.
	/// A scene cannot draw itself, even through other scenes.
	Scene {
		/// The scene drawn.
		scene: String,
	},
	/// What the program shows, transitions included, on the
	/// same frame. A scene drawing it cannot go on program, and the top layer
	/// cannot draw it, since either would draw the program into itself.
	Program,
}

impl Source {
	/// Whether the source has a picture to place in a scene.
	pub fn has_video(&self) -> bool {
		!matches!(self, Self::Microphone { .. } | Self::SystemAudio | Self::Lut { .. })
	}

	/// The file an image, media, or LUT source reads.
	pub(crate) fn path(&self) -> Option<&std::path::Path> {
		match self {
			Self::Image { path } | Self::Media { path, .. } | Self::Lut { path } => Some(path),
			_ => None,
		}
	}

	/// The file the source reads, to resolve or relativize it.
	pub(crate) fn path_mut(&mut self) -> Option<&mut PathBuf> {
		match self {
			Self::Image { path } | Self::Media { path, .. } | Self::Lut { path } => Some(path),
			_ => None,
		}
	}

	/// Whether the source opens a capture, file, or subscription of its own,
	/// rather than showing another source's or a scene.
	pub fn opens(&self) -> bool {
		!matches!(self, Self::Clone { .. } | Self::Scene { .. } | Self::Program)
	}

	/// Whether the source's picture can have its background removed: a
	/// picture of its own that may show a person.
	pub fn removes_background(&self) -> bool {
		matches!(
			self,
			Self::Camera { .. }
				| Self::Display { .. }
				| Self::Window { .. }
				| Self::Image { .. }
				| Self::Media { .. }
				| Self::Broadcast { .. }
				| Self::Browser { .. }
				| Self::Replay
		)
	}

	/// Whether the source feeds the program audio.
	pub fn has_audio(&self) -> bool {
		matches!(
			self,
			Self::Microphone { .. }
				| Self::SystemAudio
				| Self::Media { .. }
				| Self::Broadcast { .. }
				| Self::Browser { .. }
				| Self::Replay
		)
	}

	fn validate(&self, name: &str) -> Result<(), Error> {
		let (kind, width, height) = match self {
			Self::Browser { url, width, height } => {
				if url.trim().is_empty() {
					return Err(Error::Invalid(format!("browser source `{name}` has no url")));
				}
				("browser", width, height)
			}
			Self::Overlay { width, height, .. } => ("overlay", width, height),
			_ => return Ok(()),
		};
		let max = crate::browser::MAX_SIDE;
		if *width == 0 || *height == 0 || *width > max || *height > max {
			return Err(Error::Invalid(format!(
				"{kind} source `{name}` is {width}x{height}; each side must be 1 to {max} pixels"
			)));
		}
		Ok(())
	}
}

/// An audio-only scene: the sum of the sounds it holds, as their sources
/// play them, and of the audio scenes it holds. A sound it reaches more than
/// one way plays in it once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioScene {
	/// Unique among the collection's audio scenes.
	pub name: String,
	/// Whether the program broadcast carries it, as exactly one does.
	#[serde(default, skip_serializing_if = "is_false")]
	pub program: bool,
	/// Whether recordings hold it rather than the one on program. One at
	/// most is recorded.
	#[serde(default, skip_serializing_if = "is_false")]
	pub recorded: bool,
	/// The sources whose sound it holds, by name.
	#[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
	pub sounds: BTreeSet<String>,
	/// The other audio scenes it holds, by name, never itself, even through
	/// others.
	#[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
	pub scenes: BTreeSet<String>,
}

impl AudioScene {
	/// The audio scene a new collection has on program.
	pub const DEFAULT: &str = "Program";

	/// An audio scene holding nothing, neither on program nor recorded.
	pub fn new(name: impl Into<String>) -> Self {
		Self {
			name: name.into(),
			program: false,
			recorded: false,
			sounds: BTreeSet::new(),
			scenes: BTreeSet::new(),
		}
	}

	/// What a collection has without audio scenes of its own: one on
	/// program, holding nothing.
	pub fn defaults() -> Vec<Self> {
		vec![Self {
			program: true,
			..Self::new(Self::DEFAULT)
		}]
	}

	fn are_default(scenes: &Vec<Self>) -> bool {
		*scenes == Self::defaults()
	}
}

/// The gain, mute, pan, and sync delay of one source with sound, in every audio
/// scene that holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Audio {
	/// Linear gain: 1.0 is unity.
	pub gain: f32,
	/// Whether the source is silenced in every audio scene, keeping its gain.
	#[serde(default)]
	pub muted: bool,
	/// Where the source sits between left (`-1.0`) and right (`1.0`), as
	/// [`Strip::set_pan`](moq_audio::bus::Strip::set_pan) places it.
	#[serde(default, skip_serializing_if = "is_center")]
	pub pan: f32,
	/// How long the source is held back in every audio scene, so a microphone lines up
	/// with a camera that lags it. At most the bus's
	/// [`MAX_DELAY`](moq_audio::bus::MAX_DELAY).
	#[serde(default, with = "humantime_serde", skip_serializing_if = "Duration::is_zero")]
	pub delay: Duration,
}

impl Default for Audio {
	fn default() -> Self {
		Self {
			gain: 1.0,
			muted: false,
			pan: 0.0,
			delay: Duration::ZERO,
		}
	}
}

fn is_center(pan: &f32) -> bool {
	*pan == 0.0
}

/// An sRGB color with straight alpha, written `#rrggbb` or `#rrggbbaa`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SerializeDisplay, DeserializeFromStr)]
pub struct Rgba(pub [u8; 4]);

impl fmt::Display for Rgba {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let [r, g, b, a] = self.0;
		if a == u8::MAX {
			write!(f, "#{r:02x}{g:02x}{b:02x}")
		} else {
			write!(f, "#{r:02x}{g:02x}{b:02x}{a:02x}")
		}
	}
}

impl FromStr for Rgba {
	type Err = Error;

	fn from_str(text: &str) -> Result<Self, Self::Err> {
		let invalid = || Error::Invalid(format!("`{text}` is not a #rrggbb or #rrggbbaa color"));
		let hex = text.strip_prefix('#').ok_or_else(invalid)?;
		if !matches!(hex.len(), 6 | 8) || !hex.is_ascii() {
			return Err(invalid());
		}
		let mut rgba = [u8::MAX; 4];
		for (channel, pair) in rgba.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
			let pair = std::str::from_utf8(pair).map_err(|_| invalid())?;
			*channel = u8::from_str_radix(pair, 16).map_err(|_| invalid())?;
		}
		Ok(Self(rgba))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn example() -> Collection {
		let mut collection = Collection::new();
		collection.sources.insert(
			"camera".into(),
			Source::Camera {
				id: Some("/dev/video0".into()),
			},
		);
		collection.sources.insert(
			"logo".into(),
			Source::Image {
				path: "logo.png".into(),
			},
		);
		collection.sources.insert(
			"background".into(),
			Source::Color {
				color: Rgba([0x10, 0x20, 0x30, 0xff]),
			},
		);
		collection.sources.insert("mic".into(), Source::Microphone { id: None });
		collection.sources.insert(
			"guest".into(),
			Source::Broadcast {
				path: "studio/guest".into(),
			},
		);
		collection.sources.insert(
			"intro".into(),
			Source::Media {
				path: "intro.mkv".into(),
				looping: false,
				restart: true,
			},
		);
		collection.sources.insert(
			"chat".into(),
			Source::Browser {
				url: "https://example.com/chat".into(),
				width: 400,
				height: 800,
			},
		);
		// Restarting is the default, written by leaving it out.
		for (name, restart) in [("title", true), ("clock", false)] {
			collection.sources.insert(
				name.into(),
				Source::Overlay {
					html: "<h1>Live</h1>".into(),
					width: 1920,
					height: 1080,
					restart,
				},
			);
		}
		collection.audio_scenes = vec![
			AudioScene {
				program: true,
				sounds: ["mic".to_string()].into(),
				..AudioScene::new("Stream")
			},
			AudioScene {
				recorded: true,
				sounds: ["intro".to_string()].into(),
				scenes: ["Stream".to_string()].into(),
				..AudioScene::new("Recording")
			},
		];
		collection.audio.insert(
			"mic".into(),
			Audio {
				gain: 0.5,
				muted: true,
				pan: 0.25,
				delay: Duration::from_millis(80),
			},
		);
		collection.sources.insert(
			"camera faded".into(),
			Source::Clone {
				source: "camera".into(),
			},
		);
		collection.video.insert(
			"camera faded".into(),
			Video {
				filters: vec![Filter::Opacity { amount: 0.5 }],
				..Default::default()
			},
		);
		collection.top.push(Item::new("logo", Rect::fill(&collection.canvas)));

		let canvas = collection.canvas;
		let scene = &mut collection.scenes[0];
		scene.items.push(Item::new("background", Rect::fill(&canvas)));
		let rect = Rect {
			x: 100.0,
			y: 50.0,
			width: 640.0,
			height: 360.0,
		};
		scene.items.push(Item {
			visible: false,
			crop: Crop {
				left: 0.25,
				..Crop::default()
			},
			blend: Blend::Screen,
			keyframes: vec![
				Keyframe {
					x: -740.0,
					crop: Crop {
						right: 0.5,
						..Crop::default()
					},
					filters: vec![Filter::Blur { radius: 8.0 }],
					..Keyframe::new(Duration::ZERO)
				},
				Keyframe {
					ease: Ease::Out,
					filters: vec![Filter::Blur { radius: 0.0 }],
					..Keyframe::new(Duration::from_millis(400))
				},
			],
			..Item::new("camera", rect)
		});
		collection
	}

	#[test]
	fn round_trips() {
		let collection = example();
		let text = collection.to_toml().unwrap();
		assert!(text.starts_with("version = 1\n"), "{text}");
		assert_eq!(Collection::from_toml(&text).unwrap(), collection);
	}

	/// The file is meant to be read and diffed by people, so pin what it looks like.
	#[test]
	fn reads_a_handwritten_file() {
		let text = r##"
version = 1

[canvas]
width = 1280
height = 720
framerate = 60

[transition]
kind = "slide"
duration = "500ms"
direction = "up"

[sources.cam]
kind = "camera"

[sources.bg]
kind = "color"
color = "#ff000080"

[audio.cam2]
gain = 2.0
delay = "120ms"

[sources.cam2]
kind = "broadcast"
path = "room/alice"

[sources.cam-faded]
kind = "clone"
source = "cam"

[video.cam-faded]
filters = [{ kind = "opacity", amount = 0.5 }]

[sources.grade]
kind = "lut"
path = "grade.cube"

[[scenes]]
name = "Main"
items = [
	{ source = "bg", rect = { x = 0, y = 0, width = 1280, height = 720 } },
	{ source = "cam", rect = { x = 10, y = 10, width = 320, height = 180 }, visible = false, keyframes = [
		{ at = "0s", x = -330, crop = { left = 0.2 }, filters = [{ kind = "opacity", amount = 0 }] },
		{ at = "1.5s", ease = "out", filters = [{ kind = "opacity", amount = 1 }] },
	] },
	{ source = "cam-faded", rect = { x = 0, y = 0, width = 640, height = 360 }, filters = [{ kind = "opacity", amount = 0.8 }] },
	{ source = "cam", rect = { x = 0, y = 0, width = 640, height = 360 }, crop = { left = 0.1, right = 0.1 }, blend = "soft_light", filters = [
		{ kind = "chroma_key", color = "#00ff00", similarity = 0.4, smoothness = 0.08, spill = 0.1 },
		{ kind = "lut", source = "grade", amount = 0.7 },
		{ kind = "mask", source = "bg", mode = "luminance" },
		{ kind = "drop_shadow", x = 4, y = 4, blur = 8, color = "#00000080" },
	] },
]

[[scenes]]
name = "Portrait"
size = { width = 720, height = 1280 }
published = true
recorded = true
audio = "Recording"
items = [{ source = "cam", rect = { x = 0, y = 0, width = 720, height = 1280 } }]

[[audio_scenes]]
name = "Guests"
sounds = ["cam2"]

[[audio_scenes]]
name = "Stream"
program = true
scenes = ["Guests"]

[[audio_scenes]]
name = "Recording"
recorded = true
scenes = ["Stream", "Guests"]

[[top]]
source = "bg"
rect = { x = 0, y = 680, width = 1280, height = 40 }
"##;
		let collection = Collection::from_toml(text).unwrap();
		assert_eq!(collection.canvas.framerate, 60);
		assert_eq!(
			collection.transition,
			Transition::Slide {
				duration: Duration::from_millis(500),
				direction: Direction::Up
			}
		);
		assert_eq!(collection.sources["cam"], Source::Camera { id: None });
		assert_eq!(
			collection.sources["bg"],
			Source::Color {
				color: Rgba([255, 0, 0, 128])
			}
		);
		assert_eq!(
			collection.audio["cam2"],
			Audio {
				gain: 2.0,
				muted: false,
				pan: 0.0,
				delay: Duration::from_millis(120),
			}
		);
		assert_eq!(collection.program_audio(), Some("Stream"));
		assert_eq!(collection.recorded_audio(), Some("Recording"));
		assert_eq!(collection.published_audio("Portrait"), Some("Recording"));
		assert_eq!(collection.published_audio("Main"), Some("Stream"));
		// Reached two ways, played once.
		assert_eq!(collection.heard("Recording"), ["cam2"].into());
		let portrait = collection.scene("Portrait").unwrap();
		assert!(portrait.published);
		assert_eq!(collection.recorded(), Some("Portrait"));
		assert_eq!(
			collection.canvas(Some("Portrait")),
			Some(Canvas {
				width: 720,
				height: 1280,
				framerate: 60
			})
		);
		let main = collection.scene("Main").unwrap();
		assert_eq!(main.size, None, "the program's");
		assert!(main.items[0].visible);
		assert!(!main.items[1].visible);
		assert_eq!(
			main.items[1].keyframes,
			[
				Keyframe {
					x: -330.0,
					crop: Crop {
						left: 0.2,
						..Crop::default()
					},
					filters: vec![Filter::Opacity { amount: 0.0 }],
					..Keyframe::new(Duration::ZERO)
				},
				Keyframe {
					ease: Ease::Out,
					filters: vec![Filter::Opacity { amount: 1.0 }],
					..Keyframe::new(Duration::from_millis(1500))
				},
			]
		);
		assert_eq!(collection.capture("cam-faded"), "cam");
		let filters: Vec<_> = collection.filters(&main.items[2]).collect();
		assert_eq!(
			filters,
			[&Filter::Opacity { amount: 0.5 }, &Filter::Opacity { amount: 0.8 }],
			"the source's filters, then the item's"
		);
		let keyed = &main.items[3];
		assert_eq!(keyed.blend, Blend::SoftLight);
		assert_eq!(keyed.crop.right, 0.1);
		assert_eq!(
			keyed.filters[2],
			Filter::Mask {
				source: "bg".into(),
				mode: MaskMode::Luminance
			}
		);
		assert_eq!(collection.top[0].source, "bg");
	}

	/// A stinger names its media source and its cut point, and round-trips.
	#[test]
	fn reads_a_stinger() {
		let text = r#"
version = 1

[canvas]
width = 1280
height = 720
framerate = 30

[transition]
kind = "stinger"
source = "sting"
cut = "800ms"

[sources.sting]
kind = "media"
path = "sting.webm"
"#;
		let collection = Collection::from_toml(text).unwrap();
		let stinger = Transition::Stinger {
			source: "sting".into(),
			cut: Duration::from_millis(800),
		};
		assert_eq!(collection.transition, stinger);
		assert_eq!(collection.transition.stinger(), Some("sting"));
		assert_eq!(collection.transition.duration(), None);
		assert_eq!(
			Collection::from_toml(&collection.to_toml().unwrap()).unwrap(),
			collection
		);

		let missing = text.replace("source = \"sting\"", "source = \"nothing\"");
		let err = Collection::from_toml(&missing).unwrap_err().to_string();
		assert!(err.contains("`nothing`, which is not a media source"), "{err}");
	}

	#[test]
	fn refuses_another_version() {
		let text = example().to_toml().unwrap().replacen("version = 1", "version = 2", 1);
		let err = Collection::from_toml(&text).unwrap_err();
		assert!(matches!(err, Error::Version { found: 2, supported: 1 }), "{err}");

		let text = example().to_toml().unwrap().replacen("version = 1", "", 1);
		assert!(matches!(Collection::from_toml(&text), Err(Error::Format(_))));
	}

	/// A typo or a field from a newer build fails instead of being dropped on the
	/// next save.
	#[test]
	fn refuses_unknown_fields() {
		let text = example()
			.to_toml()
			.unwrap()
			.replacen("[canvas]", "[canvas]\ndepth = 3", 1);
		assert!(matches!(Collection::from_toml(&text), Err(Error::Format(_))));

		let text = example()
			.to_toml()
			.unwrap()
			.replacen("kind = \"camera\"", "kind = \"camera\"\nzoom = 2", 1);
		assert!(matches!(Collection::from_toml(&text), Err(Error::Format(_))));

		let text = example()
			.to_toml()
			.unwrap()
			.replacen("kind = \"camera\"", "kind = \"hologram\"", 1);
		assert!(matches!(Collection::from_toml(&text), Err(Error::Format(_))));
	}

	#[test]
	fn refuses_dangling_references() {
		let mut collection = example();
		collection.sources.remove("camera");
		assert!(matches!(collection.to_toml(), Err(Error::Invalid(_))));

		let mut collection = example();
		collection.audio.insert("nobody".into(), Audio::default());
		assert!(matches!(collection.validate(), Err(Error::Invalid(_))));
	}

	/// One audio scene is on program and one at most recorded, each named
	/// once, holding sources with sound and audio scenes that exist, never
	/// itself; and a source is held back no longer than the bus can.
	#[test]
	fn refuses_audio_scenes_that_do_not_add_up() {
		let refused = |edit: fn(&mut Collection)| {
			let mut collection = example();
			edit(&mut collection);
			matches!(collection.validate(), Err(Error::Invalid(_)))
		};
		fn scene<'a>(collection: &'a mut Collection, name: &str) -> &'a mut AudioScene {
			let mut scenes = collection.audio_scenes.iter_mut();
			scenes.find(|scene| scene.name == name).unwrap()
		}
		assert!(!refused(|_| {}));
		assert!(refused(|collection| scene(collection, "Stream").program = false));
		assert!(refused(|collection| scene(collection, "Recording").program = true));
		assert!(refused(|collection| scene(collection, "Stream").recorded = true));
		assert!(refused(|collection| collection
			.audio_scenes
			.push(AudioScene::new("Stream"))));
		assert!(refused(|collection| collection.audio_scenes.push(AudioScene::new(""))));
		assert!(refused(|collection| {
			scene(collection, "Stream").sounds.insert("nobody".into());
		}));
		// A picture without sound.
		assert!(refused(|collection| {
			scene(collection, "Stream").sounds.insert("camera".into());
		}));
		assert!(refused(|collection| {
			scene(collection, "Stream").scenes.insert("Monitor".into());
		}));
		assert!(refused(|collection| {
			scene(collection, "Stream").scenes.insert("Stream".into());
		}));
		// Through `Recording`, which holds `Stream`.
		assert!(refused(|collection| {
			scene(collection, "Stream").scenes.insert("Recording".into());
		}));
		assert!(refused(|collection| collection.scenes[0].audio = Some("Monitor".into())));
		assert!(!refused(
			|collection| collection.scenes[0].audio = Some("Recording".into())
		));
		assert!(refused(|collection| {
			let mic = collection.audio.get_mut("mic").unwrap();
			mic.delay = moq_audio::bus::MAX_DELAY + Duration::from_millis(1);
		}));
		assert!(!refused(|collection| {
			let mic = collection.audio.get_mut("mic").unwrap();
			mic.delay = moq_audio::bus::MAX_DELAY;
		}));
	}

	/// A clone shows another source's capture, so it must name one with a
	/// picture that is not itself a clone, and only a picture is filtered.
	#[test]
	fn refuses_a_clone_or_filter_with_nothing_to_show() {
		let clone = |collection: &mut Collection, of: &str| {
			collection
				.sources
				.insert("copy".into(), Source::Clone { source: of.into() });
		};
		let mut collection = example();
		clone(&mut collection, "camera");
		collection.validate().unwrap();

		for of in ["nobody", "mic"] {
			let mut collection = example();
			clone(&mut collection, of);
			assert!(matches!(collection.validate(), Err(Error::Invalid(_))), "{of}");
		}
		let mut collection = example();
		clone(&mut collection, "camera");
		collection
			.sources
			.insert("copy 2".into(), Source::Clone { source: "copy".into() });
		assert!(matches!(collection.validate(), Err(Error::Invalid(_))));

		let mut collection = example();
		collection.video.insert("mic".into(), Video::default());
		assert!(matches!(collection.validate(), Err(Error::Invalid(_))));

		let mut collection = example();
		collection.top.push(Item::new("nobody", Rect::fill(&collection.canvas)));
		assert!(matches!(collection.validate(), Err(Error::Invalid(_))));

		// A LUT is only looked up, and a file source needs a file.
		let mut collection = example();
		collection.sources.insert(
			"grade".into(),
			Source::Lut {
				path: "grade.cube".into(),
			},
		);
		collection.validate().unwrap();
		collection.top.push(Item::new("grade", Rect::fill(&collection.canvas)));
		assert!(matches!(collection.validate(), Err(Error::Invalid(_))));
		let media = Source::Media {
			path: "".into(),
			looping: true,
			restart: false,
		};
		for source in [
			Source::Lut { path: "".into() },
			Source::Image { path: "".into() },
			media,
		] {
			let mut collection = example();
			collection.sources.insert("blank".into(), source.clone());
			assert!(matches!(collection.validate(), Err(Error::Invalid(_))), "{source:?}");
		}
	}

	/// Only a source with a track of its own, its background kept, is
	/// published independently.
	#[test]
	fn publishes_only_a_source_with_a_track_of_its_own() {
		let mut collection = Collection::new();
		collection.sources.insert("cam".into(), Source::Camera { id: None });
		collection
			.sources
			.insert("cut".into(), Source::Clone { source: "cam".into() });
		collection.published.insert("cam".into());
		collection.validate().unwrap();

		let mut missing = collection.clone();
		missing.published.insert("gone".into());
		assert!(missing.validate().is_err());

		let mut clone = collection.clone();
		clone.published.insert("cut".into());
		assert!(clone.validate().is_err(), "a clone has no track of its own");

		let mut removed = collection.clone();
		removed.video.insert(
			"cam".into(),
			Video {
				background: Background::Remove,
				..Default::default()
			},
		);
		assert!(removed.validate().is_err(), "its track would show the background");
	}

	/// A background is removed only from a picture of the source's own that
	/// may show a person.
	#[test]
	fn removes_a_background_only_from_a_picture_of_its_own() {
		let text = r#"
version = 1

[canvas]
width = 1280
height = 720
framerate = 30

[sources.cam]
kind = "camera"

[video.cam]
background = "remove"
"#;
		let collection = Collection::from_toml(text).unwrap();
		assert_eq!(collection.video["cam"].background, Background::Remove);
		assert_eq!(
			Collection::from_toml(&collection.to_toml().unwrap()).unwrap(),
			collection
		);

		let color = Source::Color {
			color: Rgba([0, 0, 0, 255]),
		};
		for source in [Source::Clone { source: "cam".into() }, color] {
			let mut collection = collection.clone();
			collection.sources.insert("other".into(), source.clone());
			collection.video.insert("other".into(), collection.video["cam"].clone());
			assert!(matches!(collection.validate(), Err(Error::Invalid(_))), "{source:?}");
		}
	}

	#[test]
	fn refuses_a_filter_out_of_range() {
		let red = Rgba([255, 0, 0, 255]);
		let bad = [
			Filter::Opacity { amount: 1.5 },
			Filter::Opacity { amount: f32::NAN },
			Filter::Brightness { amount: -0.1 },
			Filter::Saturate { amount: f32::INFINITY },
			Filter::Invert { amount: 2.0 },
			Filter::HueRotate { degrees: f32::NAN },
			Filter::Blur { radius: -1.0 },
			Filter::DropShadow {
				x: f32::INFINITY,
				y: 0.0,
				blur: 0.0,
				color: red,
			},
			Filter::ChromaKey {
				color: red,
				similarity: 1.5,
				smoothness: 0.1,
				spill: 0.1,
			},
			Filter::LumaKey {
				min: 0.8,
				max: 0.2,
				smoothness: 0.0,
			},
		];
		for filter in bad {
			let mut collection = example();
			collection.scenes[0].items[0].filters = vec![filter.clone()];
			assert!(matches!(collection.validate(), Err(Error::Invalid(_))), "{filter:?}");
		}
	}

	/// A LUT filter names a LUT, and a mask a source with a picture of its own,
	/// since a mask draws its source's picture as it arrives.
	#[test]
	fn refuses_a_filter_reading_the_wrong_source() {
		let lut = |source: &str| Filter::Lut {
			source: source.into(),
			amount: 1.0,
		};
		let mask = |source: &str| Filter::Mask {
			source: source.into(),
			mode: MaskMode::Alpha,
		};
		let mut collection = example();
		collection.sources.insert(
			"grade".into(),
			Source::Lut {
				path: "grade.cube".into(),
			},
		);
		collection.scenes[0].items[0].filters = vec![lut("grade"), mask("logo")];
		collection.validate().unwrap();

		for filter in [
			lut("logo"),
			lut("nobody"),
			mask("grade"),
			mask("mic"),
			mask("camera faded"),
		] {
			let mut collection = collection.clone();
			collection.video.insert(
				"camera".into(),
				Video {
					filters: vec![filter.clone()],
					..Default::default()
				},
			);
			assert!(matches!(collection.validate(), Err(Error::Invalid(_))), "{filter:?}");
		}
	}

	/// Mixing two valid settings is valid, even from one end of the floats to
	/// the other, and a mix of different kinds is refused.
	#[test]
	fn filters_mix_within_their_settings() {
		let shadow = |x: f32| Filter::DropShadow {
			x,
			y: -x,
			blur: 0.0,
			color: Rgba([0, 0, 0, 255]),
		};
		for progress in [0.0, 0.25, 0.5, 1.0] {
			let mixed = shadow(-f32::MAX).toward(&shadow(f32::MAX), progress).unwrap();
			validate_filters(&[mixed], "mixed").unwrap();
			let hue = |degrees| Filter::HueRotate { degrees };
			let mixed = hue(f32::MAX).toward(&hue(-f32::MAX), progress).unwrap();
			validate_filters(&[mixed], "mixed").unwrap();
		}
		assert_eq!(mix(-f32::MAX, f32::MAX, 0.5), 0.0);
		assert_eq!(mix(f32::MAX, f32::MAX, 0.3), f32::MAX);
		let invert = Filter::Invert { amount: 1.0 };
		assert!(invert.toward(&Filter::Sepia { amount: 1.0 }, 0.5).is_none());
	}

	#[test]
	fn refuses_a_crop_that_leaves_nothing() {
		let crops = [
			Crop {
				left: 0.6,
				right: 0.4,
				..Crop::default()
			},
			Crop {
				top: -0.1,
				..Crop::default()
			},
			Crop {
				bottom: f32::NAN,
				..Crop::default()
			},
		];
		for crop in crops {
			let mut collection = example();
			collection.scenes[0].items[0].crop = crop;
			assert!(matches!(collection.validate(), Err(Error::Invalid(_))), "{crop:?}");
		}
	}

	/// A source's crop then an item's is one crop of the original picture.
	#[test]
	fn crops_compose() {
		let source = Crop {
			left: 0.5,
			..Crop::default()
		};
		let item = Crop {
			left: 0.5,
			bottom: 0.25,
			..Crop::default()
		};
		assert_eq!(
			source.then(&item),
			Crop {
				left: 0.75,
				bottom: 0.25,
				..Crop::default()
			}
		);
		assert_eq!(Crop::default().then(&item), item);
	}

	/// An item with keyframes `keyframes`, resting at (100, 50), 200x100.
	fn animated(keyframes: Vec<Keyframe>) -> Item {
		let rect = Rect {
			x: 100.0,
			y: 50.0,
			width: 200.0,
			height: 100.0,
		};
		Item {
			filters: vec![Filter::Grayscale { amount: 1.0 }],
			keyframes,
			..Item::new("camera", rect)
		}
	}

	/// Keyframes offset the item from where it rests, ease between each other,
	/// and hold the first before it and the last after it.
	#[test]
	fn keyframes_offset_the_item_and_hold_at_the_ends() {
		let item = animated(vec![
			Keyframe {
				x: -300.0,
				width: -250.0,
				crop: Crop {
					left: 0.5,
					..Crop::default()
				},
				filters: vec![Filter::Opacity { amount: 0.0 }],
				..Keyframe::new(Duration::from_secs(1))
			},
			Keyframe {
				ease: Ease::Linear,
				filters: vec![Filter::Opacity { amount: 1.0 }],
				..Keyframe::new(Duration::from_secs(2))
			},
		]);
		let at = |millis| item.posed(Duration::from_millis(millis)).into_owned();

		let first = at(0);
		assert_eq!(first, at(1000), "held before the first keyframe");
		assert_eq!(first.rect.x, -200.0);
		assert_eq!(first.rect.width, 0.0, "a size shrunk past nothing is nothing");
		assert_eq!(first.crop.left, 0.5);
		assert_eq!(
			first.filters,
			[Filter::Grayscale { amount: 1.0 }, Filter::Opacity { amount: 0.0 }],
			"after the item's own"
		);
		assert!(first.keyframes.is_empty());

		let half = at(1500);
		assert_eq!(half.rect.x, -50.0);
		assert_eq!(half.rect.width, 75.0);
		assert_eq!(half.crop.left, 0.25);
		assert_eq!(half.filters[1], Filter::Opacity { amount: 0.5 });

		let last = at(5000);
		assert_eq!(last, at(2000), "held after the last keyframe");
		assert_eq!(last.rect, item.rect);
		assert_eq!(item.settled(Duration::from_secs(5)), Settled::After);
		assert_eq!(item.settled(Duration::from_secs(2)), Settled::After);
		assert_eq!(item.settled(Duration::ZERO), Settled::Before);
		assert_eq!(
			item.settled(Duration::from_secs(1)),
			Settled::At(Duration::from_secs(1))
		);

		let still = Item::new("camera", item.rect);
		assert!(matches!(still.posed(Duration::from_secs(1)), Cow::Borrowed(_)));
	}

	/// Keyframes at the same moment jump there, so the moment itself poses the
	/// item as the later one, and before it as the earlier one.
	#[test]
	fn keyframes_at_one_moment_jump() {
		let at = Duration::from_secs(1);
		let item = animated(vec![
			Keyframe {
				x: -300.0,
				..Keyframe::new(at)
			},
			Keyframe::new(at),
		]);
		assert_eq!(item.posed(Duration::ZERO).rect.x, -200.0);
		assert_eq!(item.posed(at).rect.x, 100.0);
		assert_ne!(item.settled(Duration::ZERO), item.settled(at));
	}

	#[test]
	fn eases_start_and_land_slowly() {
		assert_eq!(Ease::Linear.apply(0.5), 0.5);
		assert_eq!(Ease::In.apply(0.5), 0.125);
		assert_eq!(Ease::Out.apply(0.5), 0.875);
		assert_eq!(Ease::InOut.apply(0.25), 0.0625);
		assert_eq!(Ease::InOut.apply(0.5), 0.5);
		for ease in Ease::ALL {
			assert_eq!((ease.apply(-1.0), ease.apply(2.0)), (0.0, 1.0), "{ease:?}");
		}
	}

	/// Filters that do not pair up by kind change halfway rather than mixing.
	#[test]
	fn unpaired_keyframe_filters_change_halfway() {
		let item = animated(vec![
			Keyframe {
				filters: vec![Filter::Sepia { amount: 1.0 }],
				..Keyframe::new(Duration::ZERO)
			},
			Keyframe {
				ease: Ease::Linear,
				filters: vec![Filter::Invert { amount: 1.0 }],
				..Keyframe::new(Duration::from_secs(1))
			},
		]);
		let filters = |millis| item.posed(Duration::from_millis(millis)).filters[1].clone();
		assert_eq!(filters(499), Filter::Sepia { amount: 1.0 });
		assert_eq!(filters(500), Filter::Invert { amount: 1.0 });
	}

	#[test]
	fn refuses_keyframes_it_cannot_play() {
		let at = |millis| Keyframe::new(Duration::from_millis(millis));
		let cases = [
			vec![at(500), at(100)],
			vec![Keyframe { x: f32::NAN, ..at(0) }],
			vec![Keyframe {
				crop: Crop {
					left: 0.6,
					right: 0.6,
					..Crop::default()
				},
				..at(0)
			}],
			vec![Keyframe {
				filters: vec![Filter::Opacity { amount: 2.0 }],
				..at(0)
			}],
			vec![Keyframe {
				filters: vec![Filter::Lut {
					source: "grade".into(),
					amount: 1.0,
				}],
				..at(0)
			}],
		];
		for keyframes in cases {
			let mut collection = example();
			collection.sources.insert(
				"grade".into(),
				Source::Lut {
					path: "grade.cube".into(),
				},
			);
			collection.scenes[0].items[0].keyframes = keyframes.clone();
			assert!(matches!(collection.validate(), Err(Error::Invalid(_))), "{keyframes:?}");
		}

		let mut collection = example();
		collection.top[0].keyframes = vec![at(0)];
		assert!(matches!(collection.validate(), Err(Error::Invalid(_))), "the top layer");
		// The same moment twice jumps.
		collection.top[0].keyframes.clear();
		collection.scenes[0].items[0].keyframes = vec![at(100), at(100)];
		collection.validate().unwrap();
	}

	#[test]
	fn refuses_duplicate_scenes() {
		let mut collection = example();
		collection.scenes.push(Scene::new("Scene"));
		assert!(matches!(collection.validate(), Err(Error::Invalid(_))));
	}

	/// `example()` with scenes `A`, `B`, and `C`, where `A` shows `B` through
	/// the scene source `b`, and `on air` a program source placed nowhere.
	fn nesting() -> Collection {
		let mut collection = example();
		let fill = Rect::fill(&collection.canvas);
		for (name, source) in [("a", "A"), ("b", "B"), ("c", "C")] {
			let scene = Source::Scene { scene: source.into() };
			collection.sources.insert(name.into(), scene);
			collection.scenes.push(Scene::new(source));
		}
		collection.sources.insert("on air".into(), Source::Program);
		collection.scenes[1].items.push(Item::new("b", fill));
		collection.validate().unwrap();
		collection
	}

	/// The scene and program sources read as written.
	#[test]
	fn reads_a_scene_source() {
		let text = "version = 1\n\n\
			[canvas]\nwidth = 1920\nheight = 1080\nframerate = 30\n\n\
			[sources.wide]\nkind = \"scene\"\nscene = \"Wide\"\n\n\
			[sources.\"on air\"]\nkind = \"program\"\n\n\
			[[scenes]]\nname = \"Wide\"\n";
		let collection = Collection::from_toml(text).unwrap();
		assert_eq!(collection.sources["wide"], Source::Scene { scene: "Wide".into() });
		assert_eq!(collection.sources["on air"], Source::Program);
		assert_eq!(
			Collection::from_toml(&collection.to_toml().unwrap()).unwrap(),
			collection
		);
		assert_eq!(collection.nested("wide"), Some(Nested::Scene("Wide")));
	}

	/// A scene source names a scene, and no scene draws itself, even through
	/// others or a clone, shown or hidden. The top layer, drawn over the
	/// program, cannot show it, nor a scene with keyframes, which it never
	/// starts, and a mask cannot read a scene.
	#[test]
	fn refuses_a_scene_drawing_itself() {
		let fill = Rect::fill(&Canvas::default());
		let refused = |edit: &dyn Fn(&mut Collection), wanted: &str| {
			let mut collection = nesting();
			edit(&mut collection);
			let err = collection.validate().unwrap_err().to_string();
			assert!(err.contains(wanted), "{err}");
		};
		refused(
			&|collection| {
				collection
					.sources
					.insert("gone".into(), Source::Scene { scene: "Gone".into() });
			},
			"`gone` draws scene `Gone`, which is not a scene",
		);
		refused(
			&|collection| {
				collection.scenes[1].items.push(Item {
					visible: false,
					..Item::new("a", fill)
				});
			},
			"scene `A` draws itself",
		);
		refused(
			&|collection| {
				collection
					.sources
					.insert("a copy".into(), Source::Clone { source: "a".into() });
				collection.scenes[2].items.push(Item::new("c", fill));
				collection.scenes[3].items.push(Item::new("a copy", fill));
			},
			"scene `A` draws itself through `B`, `C`",
		);
		refused(
			&|collection| collection.top.push(Item::new("on air", fill)),
			"the top layer places `on air`, which draws the program",
		);
		refused(
			&|collection| {
				collection.scenes[2].items.push(Item::new("on air", fill));
				collection.top.push(Item::new("a", fill));
			},
			"the top layer places `a`, which draws the program",
		);
		refused(
			&|collection| {
				let source = collection.scenes[0].items[0].source.clone();
				collection.scenes[2].items.push(Item {
					keyframes: vec![Keyframe::new(Duration::ZERO)],
					..Item::new(source, fill)
				});
				collection.top.push(Item::new("a", fill));
			},
			"with keyframes, but the top layer never goes on to start them",
		);
		refused(
			&|collection| {
				let mask = Filter::Mask {
					source: "b".into(),
					mode: MaskMode::Alpha,
				};
				collection.scenes[0].items[0].filters.push(mask);
			},
			"which is not a source with a picture of its own",
		);

		// Showing a scene is not drawing it: `B` and `C` both show `C`.
		let mut collection = nesting();
		collection.scenes[2].items.push(Item::new("c", fill));
		collection.scenes[1].items.push(Item::new("c", fill));
		collection.top.push(Item::new("c", fill));
		collection.validate().unwrap();
	}

	/// What layers draw reaches through the scenes they show, each once, and
	/// leaves hidden items out; whether a scene draws the program counts hidden
	/// ones too, since showing one is an edit away.
	#[test]
	fn drawn_reaches_through_scenes() {
		let fill = Rect::fill(&Canvas::default());
		let mut collection = nesting();
		collection.scenes[1].items.push(Item::new("b", fill));
		collection.scenes[2].items = vec![Item::new("logo", fill), Item::new("c", fill)];
		collection.scenes[3].items = vec![
			Item::new("background", fill),
			Item {
				visible: false,
				..Item::new("on air", fill)
			},
		];
		collection.validate().unwrap();
		let drawn: Vec<&str> = collection
			.drawn([Layer::scene("A")])
			.into_iter()
			.map(|item| item.source.as_str())
			.collect();
		assert_eq!(drawn, ["b", "b", "logo", "c", "background"]);
		assert!(collection.draws_program("A"));
		assert!(collection.draws_program("C"));
		assert!(!collection.draws_program("Scene"));
	}

	/// A scene's own size is one the encoder can take, a published scene is
	/// named as a path segment, since it names its broadcast, and recordings
	/// hold one scene at most. Only a scene of the program's size that draws
	/// no program goes on program.
	#[test]
	fn a_scene_has_a_size_and_outputs() {
		let mut collection = example();
		collection.scenes.push(Scene::new("a/b"));
		collection.validate().unwrap();
		collection.scenes[1].published = true;
		assert!(
			matches!(collection.validate(), Err(Error::Invalid(_))),
			"a / in a published name"
		);
		collection.scenes[1].name = r"cam\one".into();
		assert!(
			matches!(collection.validate(), Err(Error::Invalid(_))),
			r"a \ escaping nothing in a published name"
		);
		collection.scenes[1].name = "Tall".into();
		collection.scenes[1].size = Some(Size {
			width: 1081,
			height: 1920,
		});
		assert!(matches!(collection.validate(), Err(Error::Invalid(_))), "an odd width");
		collection.scenes[1].size = Some(Size {
			width: 1080,
			height: 1920,
		});
		collection.validate().unwrap();
		assert_eq!(
			collection.canvas(Some("Tall")).unwrap().size(),
			moq_video::Size::new(1080, 1920)
		);
		assert_eq!(
			collection.canvas(Some("Tall")).unwrap().framerate,
			collection.canvas.framerate
		);
		assert!(
			matches!(collection.takes("Tall"), Err(Error::Invalid(_))),
			"another size"
		);
		collection.takes("Scene").unwrap();

		assert_eq!(collection.recorded(), None, "the program");
		collection.scenes[1].recorded = true;
		assert_eq!(collection.recorded(), Some("Tall"));
		collection.scenes[0].recorded = true;
		assert!(matches!(collection.validate(), Err(Error::Invalid(_))), "two recorded");
	}

	#[test]
	fn refuses_a_canvas_the_encoder_cannot_take() {
		for (width, height, framerate) in [(1921, 1080, 30), (0, 1080, 30), (1920, 1080, 0)] {
			let mut collection = example();
			collection.canvas = Canvas {
				width,
				height,
				framerate,
			};
			assert!(
				matches!(collection.validate(), Err(Error::Invalid(_))),
				"{width}x{height}@{framerate}"
			);
		}
	}

	#[test]
	fn refuses_a_degenerate_rect() {
		let mut collection = example();
		collection.scenes[0].items[0].rect.width = f32::NAN;
		assert!(matches!(collection.validate(), Err(Error::Invalid(_))));
	}

	/// A page needs an address and a size the helper can paint and the socket
	/// can carry.
	#[test]
	fn refuses_a_degenerate_page() {
		let page = |url: &str, width, height| {
			let mut collection = example();
			collection.sources.insert(
				"chat".into(),
				Source::Browser {
					url: url.into(),
					width,
					height,
				},
			);
			collection.validate()
		};
		assert!(page("https://example.com", 4096, 1).is_ok());
		for (url, width, height) in [
			(" ", 400, 800),
			("https://example.com", 0, 800),
			("https://example.com", 400, 4097),
		] {
			assert!(
				matches!(page(url, width, height), Err(Error::Invalid(_))),
				"{url:?} {width}x{height}"
			);
		}
	}

	#[test]
	fn colors_parse_and_print() {
		assert_eq!("#102030".parse::<Rgba>().unwrap(), Rgba([0x10, 0x20, 0x30, 0xff]));
		assert_eq!("#10203040".parse::<Rgba>().unwrap().to_string(), "#10203040");
		assert_eq!(Rgba([1, 2, 3, 255]).to_string(), "#010203");
		for bad in ["102030", "#1020", "#gg0000", "#ééé"] {
			assert!(bad.parse::<Rgba>().is_err(), "{bad}");
		}
	}
}
