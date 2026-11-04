//! The scene graph, and the TOML file a [`Collection`] saves to.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_with::{DeserializeFromStr, SerializeDisplay};

use crate::Error;

/// Everything a studio composes: the canvas, the sources, and the scenes that
/// place them.
///
/// Saved with [`to_toml`](Self::to_toml) and loaded with
/// [`from_toml`](Self::from_toml). The file names devices, files, and broadcast
/// paths, never a relay URL or a token, so it is safe to commit and share.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Collection {
	/// The output every scene is drawn onto.
	pub canvas: Canvas,
	/// How a fade from preview to program behaves.
	#[serde(default)]
	pub transition: Transition,
	/// Every source, by its unique name. Scenes refer to sources by that name,
	/// so one camera can appear in many scenes while being opened once.
	#[serde(default)]
	pub sources: BTreeMap<String, Source>,
	/// Gain and mute of each audio-carrying source, by source name. A source
	/// without an entry plays at unity.
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub audio: BTreeMap<String, Audio>,
	/// The filters on each picture-carrying source, by source name, applied
	/// wherever the source is placed. A source without an entry is unfiltered.
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub video: BTreeMap<String, Video>,
	/// The scenes, in the order the scene list shows them.
	#[serde(default)]
	pub scenes: Vec<Scene>,
	/// The top layer: items drawn over whichever scene is in preview and on
	/// program, back to front, so they stay on across scene switches (a
	/// downstream keyer). They do not fade with a transition.
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
			scenes: vec![Scene::new("Scene")],
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

	/// The items of `layer`, or `None` for a scene that does not exist.
	pub fn items(&self, layer: &Layer) -> Option<&Vec<Item>> {
		match layer {
			Layer::Scene(name) => self.scene(name).map(|scene| &scene.items),
			Layer::Top => Some(&self.top),
		}
	}

	/// The items of `layer`, to edit.
	pub(crate) fn items_mut(&mut self, layer: &Layer) -> Option<&mut Vec<Item>> {
		match layer {
			Layer::Scene(name) => self
				.scenes
				.iter_mut()
				.find(|scene| &scene.name == name)
				.map(|scene| &mut scene.items),
			Layer::Top => Some(&mut self.top),
		}
	}

	/// Every layer: the scenes in order, then the top layer.
	pub fn layers(&self) -> impl Iterator<Item = (Layer, &[Item])> {
		let scenes = self
			.scenes
			.iter()
			.map(|scene| (Layer::Scene(scene.name.clone()), scene.items.as_slice()));
		scenes.chain(std::iter::once((Layer::Top, self.top.as_slice())))
	}

	/// The source whose capture `name` shows: itself, or what it clones.
	pub fn capture<'a>(&'a self, name: &'a str) -> &'a str {
		match self.sources.get(name) {
			Some(Source::Clone { source }) => source,
			_ => name,
		}
	}

	/// The filters `item` draws through: its source's, then its own.
	pub fn filters<'a>(&'a self, item: &'a Item) -> impl Iterator<Item = &'a Filter> {
		let source = self.video.get(&item.source).map(|video| video.filters.as_slice());
		source.unwrap_or_default().iter().chain(&item.filters)
	}

	/// Check what the types cannot: the canvas can be encoded, names are unique,
	/// every item, clone, audio, and video entry names a source that exists,
	/// and every crop and filter's settings are in range, each filter naming a
	/// source of the kind it reads.
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
			let on = format!("`{name}`");
			video
				.crop
				.validate()
				.map_err(|err| Error::Invalid(format!("{on}: {err}")))?;
			self.validate_chain(&video.filters, &on)?;
		}

		for (name, audio) in &self.audio {
			if !self.sources.contains_key(name) {
				return Err(Error::Invalid(format!(
					"audio settings for `{name}`, which is not a source"
				)));
			}
			if !audio.gain.is_finite() || audio.gain < 0.0 {
				return Err(Error::Invalid(format!("`{name}` has gain {}", audio.gain)));
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

/// How the program changes when the preview is faded in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
	/// How long a fade takes.
	#[serde(with = "humantime_serde")]
	pub fade: Duration,
}

impl Default for Transition {
	fn default() -> Self {
		Self {
			fade: Duration::from_millis(300),
		}
	}
}

/// One arrangement of sources on the canvas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
	/// Unique among the collection's scenes.
	pub name: String,
	/// What the scene draws, back to front.
	#[serde(default)]
	pub items: Vec<Item>,
}

impl Scene {
	/// An empty scene.
	pub fn new(name: impl Into<String>) -> Self {
		Self {
			name: name.into(),
			items: Vec::new(),
		}
	}
}

/// Where a list of items lives: a scene, or the top layer over every scene.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
	/// The scene with this name.
	Scene(String),
	/// [`Collection::top`].
	Top,
}

impl fmt::Display for Layer {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Scene(name) => write!(f, "scene `{name}`"),
			Self::Top => write!(f, "the top layer"),
		}
	}
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
	#[serde(default = "visible")]
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
}

impl Item {
	/// `source` shown, uncropped and unfiltered, at `rect`.
	pub fn new(source: impl Into<String>, rect: Rect) -> Self {
		Self {
			source: source.into(),
			rect,
			visible: true,
			crop: Crop::default(),
			filters: Vec::new(),
			blend: Blend::Normal,
		}
	}
}

fn visible() -> bool {
	true
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
	/// Trimmed before every filter and each item's own crop.
	#[serde(default, skip_serializing_if = "Crop::is_none")]
	pub crop: Crop,
	/// Applied in order, before each item's own.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub filters: Vec<Filter>,
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
}

impl Source {
	/// Whether the source has a picture to place in a scene.
	pub fn has_video(&self) -> bool {
		!matches!(self, Self::Microphone { .. } | Self::SystemAudio | Self::Lut { .. })
	}

	/// The file an image or LUT source reads.
	pub(crate) fn path(&self) -> Option<&std::path::Path> {
		match self {
			Self::Image { path } | Self::Lut { path } => Some(path),
			_ => None,
		}
	}

	/// The file the source reads, to resolve or relativize it.
	pub(crate) fn path_mut(&mut self) -> Option<&mut PathBuf> {
		match self {
			Self::Image { path } | Self::Lut { path } => Some(path),
			_ => None,
		}
	}

	/// Whether the source opens a capture, file, or subscription of its own,
	/// rather than showing another source's.
	pub fn opens(&self) -> bool {
		!matches!(self, Self::Clone { .. })
	}

	/// Whether the source feeds the program audio.
	pub fn has_audio(&self) -> bool {
		matches!(
			self,
			Self::Microphone { .. } | Self::SystemAudio | Self::Broadcast { .. } | Self::Replay
		)
	}
}

/// The gain and mute of one source in the program audio.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Audio {
	/// Linear gain: 1.0 is unity.
	pub gain: f32,
	/// Whether the source is silenced, keeping its gain.
	#[serde(default)]
	pub muted: bool,
}

impl Default for Audio {
	fn default() -> Self {
		Self {
			gain: 1.0,
			muted: false,
		}
	}
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
		collection.audio.insert("mic".into(), Audio { gain: 0.5, muted: true });
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
fade = "500ms"

[sources.cam]
kind = "camera"

[sources.bg]
kind = "color"
color = "#ff000080"

[audio.cam2]
gain = 2.0

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
	{ source = "cam", rect = { x = 10, y = 10, width = 320, height = 180 }, visible = false },
	{ source = "cam-faded", rect = { x = 0, y = 0, width = 640, height = 360 }, filters = [{ kind = "opacity", amount = 0.8 }] },
	{ source = "cam", rect = { x = 0, y = 0, width = 640, height = 360 }, crop = { left = 0.1, right = 0.1 }, blend = "soft_light", filters = [
		{ kind = "chroma_key", color = "#00ff00", similarity = 0.4, smoothness = 0.08, spill = 0.1 },
		{ kind = "lut", source = "grade", amount = 0.7 },
		{ kind = "mask", source = "bg", mode = "luminance" },
		{ kind = "drop_shadow", x = 4, y = 4, blur = 8, color = "#00000080" },
	] },
]

[[top]]
source = "bg"
rect = { x = 0, y = 680, width = 1280, height = 40 }
"##;
		let collection = Collection::from_toml(text).unwrap();
		assert_eq!(collection.canvas.framerate, 60);
		assert_eq!(collection.transition.fade, Duration::from_millis(500));
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
				muted: false
			}
		);
		let main = collection.scene("Main").unwrap();
		assert!(main.items[0].visible);
		assert!(!main.items[1].visible);
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
		for source in [Source::Lut { path: "".into() }, Source::Image { path: "".into() }] {
			let mut collection = example();
			collection.sources.insert("blank".into(), source.clone());
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

	#[test]
	fn refuses_duplicate_scenes() {
		let mut collection = example();
		collection.scenes.push(Scene::new("Scene"));
		assert!(matches!(collection.validate(), Err(Error::Invalid(_))));
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
