//! Landscape and portrait at once: while a show goes out in
//! [portrait](Collection::portrait), each scene of the program's size has a
//! portrait arrangement of the same layers, drawn on the canvas turned on its
//! side.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::collection::{Arrangement, Canvas, Collection, Crop, Item, Layer, Nested, Rect, Scene, Size, Source};
use crate::edit::{Change, Draft};

/// Where the portrait picture goes out, beneath the program's broadcast, so
/// no scene published while the show goes out in portrait takes the name.
pub const PORTRAIT: &str = "portrait";

/// How much of the portrait canvas's height the main picture takes when a
/// camera is stacked under it.
const MAIN_SHARE: f32 = 0.6;

/// How much of the landscape canvas an item covers to count as a backdrop,
/// which covers the portrait canvas too.
const BACKDROP: f32 = 0.9;

/// One of the two shapes a show goes out in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
	/// Wider than tall: the canvas as it is.
	Landscape,
	/// Taller than wide: the canvas turned on its side.
	Portrait,
}

impl Format {
	/// Both, landscape first.
	pub const ALL: [Self; 2] = [Self::Landscape, Self::Portrait];

	/// The other one.
	pub fn other(self) -> Self {
		match self {
			Self::Landscape => Self::Portrait,
			Self::Portrait => Self::Landscape,
		}
	}

	/// Its name, capitalized, as a label says it.
	pub fn name(self) -> &'static str {
		match self {
			Self::Landscape => "Landscape",
			Self::Portrait => "Portrait",
		}
	}
}

/// Where an item is drawn in the portrait picture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
	/// Where on the portrait canvas, stretched to fill it.
	pub rect: Rect,
	/// Trimmed from what its source's crop leaves, in place of the item's own
	/// crop.
	#[serde(default, skip_serializing_if = "Crop::is_none")]
	pub crop: Crop,
}

impl Placement {
	/// Where an item placed here on the `landscape` canvas sits on the
	/// portrait one without a place of its own there: as far across and down,
	/// scaled by as much as the narrower side shrinks, so it keeps its shape
	/// and fits, its picture `turn` times as wide for its height there, as
	/// [`Collection::turn`] says.
	fn fitted(self, landscape: Canvas, turn: f32) -> Self {
		let (from, to) = (Rect::fill(&landscape), Rect::fill(&landscape.turned()));
		self.moved(&from, &to, turn)
	}

	/// Moved from on `from` to on `to` as [`fitted`](Self::fitted) moves it
	/// from one canvas to the other: its center as far across and down, and
	/// scaled to fit, uncropped, as tall again as a picture `turn` times as
	/// wide for its height needs.
	fn moved(self, from: &Rect, to: &Rect, turn: f32) -> Self {
		let rect = self.rect;
		let scale = (to.width / from.width).min(to.height / from.height);
		let (width, height) = (rect.width * scale, rect.height * scale / turn);
		let center = (
			to.x + (rect.x + rect.width / 2.0 - from.x) / from.width * to.width,
			to.y + (rect.y + rect.height / 2.0 - from.y) / from.height * to.height,
		);
		Self {
			rect: Rect {
				x: (center.0 - width / 2.0).round(),
				y: (center.1 - height / 2.0).round(),
				width: width.round(),
				height: height.round(),
			},
			crop: self.crop,
		}
	}
}

/// How a scene is arranged in the portrait picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Portrait {
	/// Where its items in a slot go: in a row under the main one unless set,
	/// since a column beside it leaves a tall canvas mostly empty.
	#[serde(default = "row", skip_serializing_if = "is_row")]
	pub arrangement: Arrangement,
}

impl Default for Portrait {
	fn default() -> Self {
		Self {
			arrangement: Arrangement::Row,
		}
	}
}

impl Portrait {
	pub(crate) fn is_default(&self) -> bool {
		*self == Self::default()
	}
}

fn row() -> Arrangement {
	Arrangement::Row
}

fn is_row(arrangement: &Arrangement) -> bool {
	*arrangement == Arrangement::Row
}

impl Canvas {
	/// The canvas turned on its side: its width and height swapped.
	pub fn turned(self) -> Self {
		Self {
			width: self.height,
			height: self.width,
			framerate: self.framerate,
		}
	}
}

impl Item {
	/// Whether it is shown in `format`'s picture.
	pub fn shown_in(&self, format: Format) -> bool {
		self.format.is_none_or(|only| only == format)
	}

	/// The item drawn at `placement` in `format`'s picture, as it was in the
	/// other. An item in a slot has one crop in both, which this sets.
	pub fn placed(self, format: Format, placement: Placement) -> Self {
		match format {
			Format::Portrait if self.slot.is_none() => Self {
				portrait: Some(placement),
				..self
			},
			_ => Self {
				rect: placement.rect,
				crop: placement.crop,
				..self
			},
		}
	}

	/// Where it is drawn in the landscape picture.
	fn landscaped(&self) -> Placement {
		Placement {
			rect: self.rect,
			crop: self.crop,
		}
	}

	/// Whether it says anything about the portrait picture.
	fn formats(&self) -> bool {
		self.format.is_some() || self.portrait.is_some()
	}
}

impl Collection {
	/// The portrait picture's canvas, while the show goes out in portrait.
	pub fn portrait_canvas(&self) -> Option<Canvas> {
		self.portrait.then(|| self.canvas.turned())
	}

	/// The canvas `format`'s picture is drawn on: the program's, or turned.
	pub fn canvas_of(&self, format: Format) -> Canvas {
		match format {
			Format::Landscape => self.canvas,
			Format::Portrait => self.canvas.turned(),
		}
	}

	/// Where `item` is drawn in `format`'s picture: its rect and crop, or its
	/// portrait place, or failing that where it sits on the landscape picture,
	/// fitted, its picture turned as [`turn`](Self::turn) says.
	pub fn placement(&self, item: &Item, format: Format) -> Placement {
		match format {
			Format::Landscape => item.landscaped(),
			Format::Portrait => item
				.portrait
				.unwrap_or_else(|| item.landscaped().fitted(self.canvas, self.turn(item))),
		}
	}

	/// `item` as `format`'s picture draws it at rest: at its place there, and
	/// hidden unless shown in it. An item in a slot stays where the
	/// arrangement puts it.
	pub fn portrayed(&self, item: &Item, format: Format) -> Item {
		let mut portrayed = item.clone();
		if item.slot.is_none() {
			let placement = self.placement(item, format);
			portrayed.rect = placement.rect;
			portrayed.crop = placement.crop;
		}
		portrayed.visible &= item.shown_in(format);
		portrayed
	}

	/// How many times as wide for its height `item`'s picture is in the
	/// portrait picture as in the landscape one: narrower, as the canvas
	/// turns, for one showing the program or a scene drawn in both, which
	/// shows its portrait picture there; as wide for any other.
	pub fn turn(&self, item: &Item) -> f32 {
		let turns = match self.nested(&item.source) {
			Some(Nested::Program) => self.portrait,
			Some(Nested::Scene(scene)) => self.dual(&Layer::scene(scene)),
			None => false,
		};
		match turns {
			true => (self.canvas.height as f32 / self.canvas.width as f32).powi(2),
			false => 1.0,
		}
	}

	/// Whether `layer` is drawn in both pictures: the top layer, or a scene of
	/// the program's size, while the show goes out in portrait.
	pub fn dual(&self, layer: &Layer) -> bool {
		self.portrait
			&& match &layer.scene {
				None => true,
				Some(scene) => self.scene(scene).is_some_and(|scene| scene.size.is_none()),
			}
	}

	/// The collection as `format`'s picture draws it, while the show goes out
	/// in portrait: in portrait, on the canvas turned, each scene of the
	/// program's size arranged as its portrait arrangement says and each item
	/// at its portrait place; in either, an item shown only in the other is
	/// hidden. A scene of its own size is drawn as it is. Itself otherwise.
	pub fn formatted(&self, format: Format) -> Cow<'_, Self> {
		if !self.portrait {
			return Cow::Borrowed(self);
		}
		if format == Format::Landscape {
			let hides = self
				.layers()
				.any(|(_, items)| items.iter().any(|item| !item.shown_in(format)));
			if !hides {
				return Cow::Borrowed(self);
			}
		}
		let mut view = self.clone();
		view.canvas = self.canvas_of(format);
		for (scene, at) in view.scenes.iter_mut().zip(&self.scenes) {
			if scene.size.is_some() {
				continue;
			}
			if format == Format::Portrait {
				scene.arrangement = scene.portrait.arrangement;
			}
			for (item, at) in scene.items.iter_mut().zip(&at.items) {
				*item = self.portrayed(at, format);
			}
		}
		for (item, at) in view.top.iter_mut().zip(&self.top) {
			*item = self.portrayed(at, format);
		}
		Cow::Owned(view)
	}

	/// The picture `layer` is edited in when `format` is asked for: `format`
	/// while the layer is drawn in both, landscape otherwise.
	pub fn format_of(&self, layer: &Layer, format: Format) -> Format {
		match self.dual(layer) {
			true => format,
			false => Format::Landscape,
		}
	}

	/// Check the portrait picture's rules: only a landscape canvas turns, and
	/// only a scene of the program's size, or the top layer, says anything
	/// about it, an item in a slot or shown only in landscape taking no place
	/// there; nor does a scene publish where it goes out.
	pub(crate) fn validate_formats(&self) -> Result<(), Error> {
		if self.portrait && self.canvas.width <= self.canvas.height {
			return Err(Error::Invalid(format!(
				"the show goes out in portrait too, which turns a landscape canvas, not {}x{}",
				self.canvas.width, self.canvas.height
			)));
		}
		for scene in &self.scenes {
			let why = match (self.portrait, scene.size) {
				(false, _) => "the show does not go out in portrait",
				(true, Some(_)) => "it has a size of its own, so it is drawn in one format",
				(true, None) => "",
			};
			if why.is_empty() {
				continue;
			}
			if !scene.portrait.is_default() {
				return Err(Error::Invalid(format!(
					"scene `{}` has a portrait arrangement, but {why}",
					scene.name
				)));
			}
			if let Some(item) = scene.items.iter().find(|item| item.formats()) {
				return Err(Error::Invalid(format!(
					"`{}` in scene `{}` says where it shows in portrait, but {why}",
					item.source, scene.name
				)));
			}
		}
		if !self.portrait
			&& let Some(item) = self.top.iter().find(|item| item.formats())
		{
			return Err(Error::Invalid(format!(
				"always-on-top `{}` says where it shows in portrait, but the show does not go out in portrait",
				item.source
			)));
		}
		for (layer, items) in self.layers() {
			for item in items {
				let Some(placement) = &item.portrait else { continue };
				let on = format!("`{}` in {layer}", item.source);
				if item.slot.is_some() {
					return Err(Error::Invalid(format!(
						"{on} is in a slot, which places it in portrait too, so it takes no portrait place"
					)));
				}
				if item.format == Some(Format::Landscape) {
					return Err(Error::Invalid(format!(
						"{on} is shown only in landscape, so it takes no portrait place"
					)));
				}
				placement.rect.validate(&layer, &item.source)?;
				placement
					.crop
					.validate()
					.map_err(|err| Error::Invalid(format!("{on} in portrait: {err}")))?;
			}
		}
		if self.portrait
			&& let Some(scene) = self
				.scenes
				.iter()
				.find(|scene| scene.published && scene.name == PORTRAIT)
		{
			return Err(Error::Invalid(format!(
				"scene `{}` is published where the portrait picture goes out; rename it to publish it",
				scene.name
			)));
		}
		Ok(())
	}

	/// The items of a layer as a show turning to portrait first places them:
	/// the main picture, the largest covering a quarter of the canvas or more,
	/// cropped to the top of the portrait canvas, or all of it without a
	/// camera, and the largest camera stacked under it, keeping its shape if
	/// it has an outline; a backdrop covering the landscape canvas covers the
	/// portrait one, cropped, or, a page or overlay, which is mostly
	/// see-through, is fitted whole, never the main picture. What sits on the
	/// main picture keeps its place on it. The rest sit where they are, fitted, until moved there. A layer
	/// showing a scene drawn in both, or the program, shows its portrait
	/// picture, shaped for the portrait canvas: covering the landscape canvas,
	/// it covers the portrait one whole, rather than being the main picture.
	fn turned_on(&self, items: &[Item]) -> Vec<Item> {
		let landscape = self.canvas;
		let portrait = landscape.turned();
		let whole = Rect::fill(&portrait);
		let canvas = landscape.width as f32 * landscape.height as f32;
		let area = |item: &Item| covered(&item.rect, &landscape) / canvas;
		let free: Vec<usize> = (0..items.len())
			.filter(|&at| {
				let item = &items[at];
				item.slot.is_none() && item.visible && item.shown_in(Format::Portrait) && area(item) > 0.0
			})
			.collect();
		let largest = |among: &mut dyn Iterator<Item = usize>| {
			// The frontmost of two as large, which is the one seen.
			among.max_by(|a, b| area(&items[*a]).total_cmp(&area(&items[*b])).then(a.cmp(b)))
		};
		let source = |at: usize| self.sources.get(self.capture(&items[at].source));
		// A page or overlay is mostly see-through, so it is fitted whole
		// rather than cropped.
		let see_through = |at: usize| matches!(source(at), Some(Source::Browser { .. } | Source::Overlay { .. }));
		let turned = |at: usize| self.turn(&items[at]) != 1.0;
		// One covering the landscape canvas that is see-through, or shows its
		// own portrait picture, covers or fits the portrait canvas whole
		// rather than being cropped to the main picture.
		let whole_page = |at: usize| area(&items[at]) >= BACKDROP && (turned(at) || see_through(at));
		let main =
			largest(&mut free.iter().copied().filter(|&at| !whole_page(at))).filter(|&at| area(&items[at]) >= 0.25);
		let camera = largest(&mut free.iter().copied().filter(|&at| {
			Some(at) != main && matches!(source(at), Some(Source::Camera { .. })) && area(&items[at]) < BACKDROP
		}));

		let mut placed = items.to_vec();
		if let Some(main) = main {
			let top = match camera {
				Some(_) => Rect {
					height: (portrait.height as f32 * MAIN_SHARE).round(),
					..whole
				},
				None => whole,
			};
			placed[main].portrait = Some(self.covering(&items[main], top));
			// What sits on the main picture, such as a lower third, keeps
			// its place on it, rather than landing on the camera.
			let on = items[main].rect;
			for &at in free.iter().filter(|&&at| Some(at) != Some(main) && Some(at) != camera) {
				let item = &items[at];
				let center = (
					item.rect.x + item.rect.width / 2.0,
					item.rect.y + item.rect.height / 2.0,
				);
				let inside =
					(on.x..on.x + on.width).contains(&center.0) && (on.y..on.y + on.height).contains(&center.1);
				if camera.is_some() && inside && area(item) < BACKDROP {
					placed[at].portrait = Some(item.landscaped().moved(&on, &top, self.turn(item)));
				}
			}
			if let Some(camera) = camera {
				let under = Rect {
					y: top.height,
					height: whole.height - top.height,
					..whole
				};
				let item = &items[camera];
				placed[camera].portrait = Some(match item.shape.is_square() {
					true => self.covering(item, under),
					false => self.fitting(item, under),
				});
			}
		}
		for &at in &free {
			let item = &items[at];
			if Some(at) != main && Some(at) != camera && area(item) >= BACKDROP && !see_through(at) {
				placed[at].portrait = Some(self.covering(item, whole));
			}
		}
		placed
	}
}

impl Scene {
	/// The scene at `size`, its own, or the program's for `None`. A scene of
	/// its own size is drawn in one picture, so it keeps no portrait
	/// arrangement or place, and its layers shown only in portrait are
	/// removed, as turning portrait off removes them.
	pub fn sized(mut self, size: Option<Size>) -> Self {
		if size.is_some() {
			self.single();
		}
		Self { size, ..self }
	}

	/// Forget the portrait picture: its arrangement, each layer's place
	/// there, and the layers shown only there.
	fn single(&mut self) {
		self.portrait = Portrait::default();
		self.items.retain(|item| item.shown_in(Format::Landscape));
		for item in &mut self.items {
			item.format = None;
			item.portrait = None;
		}
	}
}

impl Draft {
	/// Give the scene at `index` a size of its own, or the program's with
	/// `None`, as [`Scene::sized`] does. While the show goes out in both, a
	/// layer elsewhere showing it shows its portrait picture as long as it
	/// follows the program's size, and its own size otherwise, so one placed
	/// in the portrait picture, whose picture changes shape there, is fitted
	/// where it was placed, as wide for its height as the picture became.
	pub fn set_size(&mut self, index: usize, size: Option<Size>) -> Result<(), Error> {
		let before = self.collection().clone();
		let scene = self.scene_at(index)?;
		let name = scene.name.clone();
		self.set_scene(index, scene.sized(size))?;
		let after = self.collection().clone();
		let (Some(from), Some(to)) = (before.portrait_shape_of(&name), after.portrait_shape_of(&name)) else {
			return Ok(());
		};
		if from == to {
			return Ok(());
		}
		for (layer, items) in after.layers() {
			for (index, item) in items.iter().enumerate() {
				let Some(placement) = item.portrait else { continue };
				// A place shrunk to nothing has no shape to keep.
				if placement.rect.width <= 0.0 || placement.rect.height <= 0.0 {
					continue;
				}
				if after.nested(&item.source) != Some(Nested::Scene(&name)) {
					continue;
				}
				// Cropped as it was, the picture shown there changes shape as
				// the whole does.
				let shape = placement.rect.width / placement.rect.height * to / from;
				let placement = Placement {
					rect: fit(shape, placement.rect),
					crop: placement.crop,
				};
				let item = Item {
					portrait: Some(placement),
					..item.clone()
				};
				self.set_item(&layer, index, item)?;
			}
		}
		Ok(())
	}

	/// Send the show out in portrait too, or no longer. Turned on, each
	/// layer's portrait arrangement starts from its landscape one: the main
	/// picture cropped to the top, the camera stacked under it, and slots in
	/// a row unless they were in a grid or inset. A scene given the program's
	/// size as its own follows the program's instead, so it has a portrait
	/// picture and can stay on program. Turned off, the layers shown only in
	/// portrait are removed and every portrait place dropped.
	pub fn set_portrait(&mut self, on: bool) -> Result<(), Error> {
		let before = self.collection().portrait;
		if before == on {
			return Ok(());
		}
		let collection = self.collection().clone();
		let program = (collection.canvas.width, collection.canvas.height);
		let pinned = |scene: &Scene| on && scene.size.is_some_and(|size| (size.width, size.height) == program);
		// The show as it goes out in both, so a layer showing a scene that
		// will be drawn in both is placed for its portrait picture.
		let mut turning = collection.clone();
		turning.portrait = true;
		for scene in turning.scenes.iter_mut().filter(|scene| pinned(scene)) {
			scene.size = None;
		}
		for (index, scene) in collection.scenes.iter().enumerate() {
			if scene.size.is_some() && !pinned(scene) {
				continue;
			}
			let mut after = Scene {
				size: None,
				..scene.clone()
			};
			match on {
				true => {
					after.items = turning.turned_on(&scene.items);
					if matches!(scene.arrangement, Arrangement::Grid | Arrangement::Inset) {
						after.portrait.arrangement = scene.arrangement;
					}
				}
				false => after.single(),
			}
			if after != *scene {
				self.set_scene(index, after)?;
			}
		}
		let top = Layer::top();
		match on {
			true => {
				for (index, item) in turning.turned_on(&collection.top).into_iter().enumerate() {
					if item != collection.top[index] {
						self.set_item(&top, index, item)?;
					}
				}
			}
			false => {
				for (index, item) in collection.top.iter().enumerate().rev() {
					if !item.shown_in(Format::Landscape) {
						self.remove_item(&top, index)?;
					} else if item.formats() {
						let item = Item {
							format: None,
							portrait: None,
							..item.clone()
						};
						self.set_item(&top, index, item)?;
					}
				}
			}
		}
		self.change(Change::Portrait { before, after: on })
	}
}

/// How much of `canvas` `rect` covers, in square pixels.
fn covered(rect: &Rect, canvas: &Canvas) -> f32 {
	let width = (rect.x + rect.width).min(canvas.width as f32) - rect.x.max(0.0);
	let height = (rect.y + rect.height).min(canvas.height as f32) - rect.y.max(0.0);
	width.max(0.0) * height.max(0.0)
}

impl Collection {
	/// The shape `item`'s picture shows in the portrait picture, as its rect
	/// shows it in the landscape one, turned as [`turn`](Self::turn) says.
	fn portrait_shape(&self, item: &Item) -> f32 {
		item.rect.width / item.rect.height * self.turn(item)
	}

	/// `item` covering `rect` in the portrait picture, its picture cropped
	/// evenly to `rect`'s shape on top of its own crop, so it shows
	/// unstretched as it did.
	fn covering(&self, item: &Item, rect: Rect) -> Placement {
		let (shown, wanted) = (self.portrait_shape(item), rect.width / rect.height);
		// A shape turned to the canvas's own, in floating point, trims nothing.
		let trim = match (shown / wanted - 1.0).abs() < 1e-4 {
			true => Crop::default(),
			false => Crop::covering(shown, wanted),
		};
		Placement {
			rect,
			crop: item.crop.then(&trim),
		}
	}

	/// `item` as large as fits in `rect` in the portrait picture, keeping its
	/// shape, centered.
	fn fitting(&self, item: &Item, rect: Rect) -> Placement {
		Placement {
			rect: fit(self.portrait_shape(item), rect),
			crop: item.crop,
		}
	}

	/// How many times as wide for its height the portrait picture draws the
	/// scene called `scene`, whole: as the turned canvas while it is drawn in
	/// both, as its own size otherwise; `None` while the show does not go out
	/// in portrait, or the scene is gone.
	fn portrait_shape_of(&self, scene: &str) -> Option<f32> {
		let canvas = match self.dual(&Layer::scene(scene)) {
			true => self.portrait_canvas()?,
			false => self.portrait.then(|| self.canvas(Some(scene))).flatten()?,
		};
		Some(canvas.width as f32 / canvas.height as f32)
	}
}

/// As large as a picture `shape` times as wide for its height fits in
/// `rect`, centered.
fn fit(shape: f32, rect: Rect) -> Rect {
	let width = rect.width.min(rect.height * shape);
	let height = width / shape;
	Rect {
		x: (rect.x + (rect.width - width) / 2.0).round(),
		y: (rect.y + (rect.height - height) / 2.0).round(),
		width: width.round(),
		height: height.round(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::collection::{Rgba, Scene, Shape, Size, Slot};

	fn color() -> Source {
		Source::Color {
			color: Rgba([255, 0, 0, 255]),
		}
	}

	/// A 1920x1080 show: a game filling the canvas, a camera in its corner,
	/// and a logo, in scene "Game"; and a slotted scene "Guests".
	fn show() -> Collection {
		let mut collection = Collection::new();
		collection.sources.insert("game".into(), color());
		collection.sources.insert("camera".into(), Source::Camera { id: None });
		collection.sources.insert("logo".into(), color());
		let fill = Rect::fill(&collection.canvas);
		let corner = Rect {
			x: 1440.0,
			y: 720.0,
			width: 480.0,
			height: 270.0,
		};
		let logo = Rect {
			x: 1600.0,
			y: 40.0,
			width: 280.0,
			height: 120.0,
		};
		let slotted = |slot| Item {
			slot: Some(slot),
			..Item::new("camera", Rect::default())
		};
		collection.scenes = vec![
			Scene {
				items: vec![
					Item::new("game", fill),
					Item::new("camera", corner),
					Item::new("logo", logo),
				],
				..Scene::new("Game")
			},
			Scene {
				items: vec![slotted(Slot::Main), slotted(Slot::Tile)],
				..Scene::new("Guests")
			},
		];
		collection.validate().unwrap();
		collection
	}

	fn turned(collection: &Collection) -> Collection {
		let mut draft = collection.draft("Both");
		draft.set_portrait(true).unwrap();
		let edit = draft.finish().unwrap();
		let mut turned = collection.clone();
		turned.apply(&edit).unwrap();
		turned
	}

	#[test]
	fn turning_on_crops_the_main_picture_over_the_camera() {
		let turned = turned(&show());
		assert!(turned.portrait);
		let items = &turned.scenes[0].items;
		let game = items[0].portrait.unwrap();
		assert_eq!(
			game.rect,
			Rect {
				x: 0.0,
				y: 0.0,
				width: 1080.0,
				height: 1152.0
			}
		);
		// The 16:9 game cropped evenly to the 1080x1152 band, unstretched.
		let kept = 1.0 - game.crop.left - game.crop.right;
		assert!((1920.0 * kept / 1080.0 - 1080.0 / 1152.0).abs() < 1e-3, "{game:?}");
		assert!((game.crop.left - game.crop.right).abs() < 1e-6);
		let camera = items[1].portrait.unwrap();
		assert_eq!(
			camera.rect,
			Rect {
				x: 0.0,
				y: 1152.0,
				width: 1080.0,
				height: 768.0
			}
		);
		// The logo on the game keeps its place on it, above the camera.
		assert_eq!(
			items[2].portrait.unwrap().rect,
			Rect {
				x: 900.0,
				y: 73.0,
				width: 158.0,
				height: 68.0
			}
		);
		turned.validate().unwrap();
	}

	/// A backdrop as large as the game, behind it, is not the main picture:
	/// the game in front is, and the backdrop covers the portrait canvas.
	/// A layer showing a scene drawn in both shows its portrait picture,
	/// 9:16, in the portrait one: covering the landscape canvas, such as a
	/// scene of overlays, it covers the portrait canvas whole rather than
	/// being the main picture; a smaller one is shaped for it wherever it
	/// goes, set or fitted. A scene of its own size shows one picture.
	#[test]
	fn a_scene_source_is_placed_for_its_portrait_picture() {
		let mut show = show();
		show.sources
			.insert("overlays".into(), Source::Scene { scene: "Guests".into() });
		show.sources
			.insert("wide".into(), Source::Scene { scene: "Wide".into() });
		show.scenes.push(Scene {
			size: Some(Size {
				width: 1280,
				height: 720,
			}),
			..Scene::new("Wide")
		});
		let fill = Rect::fill(&show.canvas);
		let pip = Rect {
			x: 0.0,
			y: 0.0,
			width: 960.0,
			height: 540.0,
		};
		let items = &mut show.scenes[0].items;
		items.push(Item::new("overlays", fill));
		items.push(Item::new("wide", pip));
		show.validate().unwrap();
		let turned = turned(&show);
		let items = &turned.scenes[0].items;
		let whole = Rect::fill(&show.canvas.turned());
		assert_eq!(
			items[0].portrait.unwrap().rect.height,
			1152.0,
			"the game is the main picture"
		);
		let overlays = items[3].portrait.unwrap();
		assert_eq!((overlays.rect, overlays.crop), (whole, Crop::default()));
		// On the game, the scene of its own size keeps its 16:9 shape.
		let wide = items[4].portrait.unwrap().rect;
		assert!((wide.width / wide.height - 16.0 / 9.0).abs() < 0.01, "{wide:?}");

		// Without a place of its own, a scene drawn in both is fitted turned:
		// a quarter of the landscape canvas is a quarter of the portrait one.
		let quarter = Item::new("overlays", pip);
		let fitted = turned.placement(&quarter, Format::Portrait).rect;
		assert_eq!(
			fitted,
			Rect {
				x: 0.0,
				y: 0.0,
				width: 540.0,
				height: 960.0
			}
		);
		let program = Item::new("program", pip);
		let mut with_program = turned.clone();
		with_program.sources.insert("program".into(), Source::Program);
		assert_eq!(with_program.placement(&program, Format::Portrait).rect, fitted);
		assert_eq!(turned.turn(&Item::new("wide", pip)), 1.0);
		assert_eq!(turned.turn(&Item::new("camera", pip)), 1.0);
	}

	#[test]
	fn the_frontmost_of_two_as_large_is_the_main_picture() {
		let mut show = show();
		let fill = show.scenes[0].items[0].rect;
		show.scenes[0].items.insert(0, Item::new("logo", fill));
		let turned = turned(&show);
		let items = &turned.scenes[0].items;
		assert_eq!(items[0].portrait.unwrap().rect, Rect::fill(&show.canvas.turned()));
		assert_eq!(items[1].portrait.unwrap().rect.height, 1152.0);
	}

	#[test]
	fn a_round_camera_keeps_its_shape() {
		let mut show = show();
		show.scenes[0].items[1].rect = Rect {
			x: 1500.0,
			y: 700.0,
			width: 300.0,
			height: 300.0,
		};
		show.scenes[0].items[1].shape = Shape {
			corners: [0.5; 4],
			..Shape::default()
		};
		let turned = turned(&show);
		let camera = turned.scenes[0].items[1].portrait.unwrap();
		assert_eq!(camera.rect.width, camera.rect.height);
		assert_eq!(camera.rect.width, 768.0);
		assert_eq!(camera.rect.y, 1152.0);
		assert_eq!(camera.rect.x, 156.0);
	}

	#[test]
	fn without_a_camera_the_main_picture_covers_the_canvas() {
		let mut show = show();
		show.scenes[0].items.remove(1);
		let turned = turned(&show);
		let game = turned.scenes[0].items[0].portrait.unwrap();
		assert_eq!(game.rect, Rect::fill(&show.canvas.turned()));
	}

	#[test]
	fn slots_go_in_a_row_unless_gridded() {
		let mut show = show();
		show.scenes[1].arrangement = Arrangement::Grid;
		let mut copy = show.scenes[1].clone();
		copy.name = "Row".into();
		for item in &mut copy.items {
			item.id = crate::Id::fresh();
		}
		copy.arrangement = Arrangement::Column;
		show.scenes.push(copy);
		let turned = turned(&show);
		assert_eq!(turned.scenes[1].portrait.arrangement, Arrangement::Grid);
		assert_eq!(turned.scenes[2].portrait.arrangement, Arrangement::Row);
		assert!(turned.scenes[1].items.iter().all(|item| item.portrait.is_none()));
	}

	#[test]
	fn a_scene_of_its_own_size_is_left_alone() {
		let mut show = show();
		show.scenes[0].size = Some(Size {
			width: 1080,
			height: 1920,
		});
		let turned = turned(&show);
		assert_eq!(turned.scenes[0], show.scenes[0]);
	}

	/// A scene of its own size has no portrait picture, so it does not go on
	/// program while the show goes out in both, even at the program's size;
	/// one given the program's size before turning on follows it instead.
	#[test]
	fn only_a_scene_following_the_program_goes_on_in_both() {
		let mut show = show();
		let program = Size {
			width: 1920,
			height: 1080,
		};
		show.scenes[0].size = Some(program);
		show.takes("Game").unwrap();
		let mut turned = turned(&show);
		assert_eq!(
			turned.scenes[0].size, None,
			"pinned at the program's size, so it follows it"
		);
		assert!(turned.scenes[0].items[0].portrait.is_some());
		turned.takes("Game").unwrap();

		turned.scenes[0] = turned.scenes[0].clone().sized(Some(program));
		turned.validate().unwrap();
		let why = turned.refuses("Game").unwrap();
		assert!(why.contains("size of its own"), "{why}");
	}

	#[test]
	fn the_portrait_view_places_each_item_there() {
		let mut turned = turned(&show());
		turned.scenes[0].items[2].format = Some(Format::Landscape);
		let view = turned.formatted(Format::Portrait);
		assert_eq!(view.canvas, turned.canvas.turned());
		let items = &view.scenes[0].items;
		assert_eq!(items[0].rect, turned.scenes[0].items[0].portrait.unwrap().rect);
		assert!(!items[2].visible, "shown only in landscape");
		assert_eq!(view.scenes[1].arrangement, Arrangement::Row);
		// The landscape view hides nothing shown in landscape, so it is the
		// collection itself.
		assert!(matches!(turned.formatted(Format::Landscape), Cow::Borrowed(_)));
		turned.scenes[0].items[2].format = Some(Format::Portrait);
		let view = turned.formatted(Format::Landscape);
		assert!(!view.scenes[0].items[2].visible);
		assert!(view.scenes[0].items[0].visible);
	}

	#[test]
	fn fitted_keeps_its_place_and_shape() {
		let landscape = Canvas::default();
		let logo = Rect {
			x: 1600.0,
			y: 40.0,
			width: 280.0,
			height: 120.0,
		};
		let fitted = Placement {
			rect: logo,
			crop: Crop::default(),
		}
		.fitted(landscape, 1.0);
		// 9/16 the size, its center as far across and down.
		assert_eq!(fitted.rect.width, 158.0);
		assert_eq!(fitted.rect.height, 68.0);
		let center = fitted.rect.x + fitted.rect.width / 2.0;
		assert!((center / 1080.0 - 1740.0 / 1920.0).abs() < 1e-3);
		let center = fitted.rect.y + fitted.rect.height / 2.0;
		assert!((center / 1920.0 - 100.0 / 1080.0).abs() < 1e-3);
	}

	#[test]
	fn turning_off_drops_the_portrait_picture() {
		let mut turned = turned(&show());
		turned.scenes[0].items.push(Item {
			format: Some(Format::Portrait),
			..Item::new("logo", Rect::fill(&turned.canvas))
		});
		turned.top.push(Item {
			format: Some(Format::Portrait),
			..Item::new("logo", Rect::fill(&turned.canvas))
		});
		turned.top.push(Item {
			format: Some(Format::Landscape),
			..Item::new("logo", Rect::fill(&turned.canvas))
		});
		turned.validate().unwrap();
		let mut draft = turned.draft("Landscape");
		draft.set_portrait(false).unwrap();
		let edit = draft.finish().unwrap();
		let mut off = turned.clone();
		off.apply(&edit).unwrap();
		assert_eq!(off.scenes, show().scenes);
		assert_eq!(off.top.len(), 1);
		assert_eq!(off.top[0].format, None);
		// Undone whole.
		off.apply(&edit.inverse()).unwrap();
		assert_eq!(off, turned);
	}

	#[test]
	fn refuses_what_the_portrait_picture_cannot_draw() {
		let refused = |edit: &dyn Fn(&mut Collection)| {
			let mut collection = turned(&show());
			edit(&mut collection);
			collection.validate().unwrap_err().to_string()
		};
		assert!(refused(&|c| c.canvas = c.canvas.turned()).contains("turns a landscape canvas"));
		assert!(refused(&|c| c.scenes[1].items[0].portrait = Some(Placement::default())).contains("in a slot"));
		assert!(refused(&|c| c.scenes[0].items[0].format = Some(Format::Landscape)).contains("only in landscape"));
		assert!(
			refused(&|c| {
				c.portrait = false;
			})
			.contains("does not go out in portrait")
		);
		assert!(
			refused(&|c| {
				c.scenes[0].name = PORTRAIT.into();
				c.scenes[0].published = true;
			})
			.contains("rename it")
		);
	}

	/// A scene given its own size is drawn in one picture, so it forgets the
	/// portrait one in the same edit, which validates.
	#[test]
	fn a_scene_given_its_own_size_forgets_the_portrait_picture() {
		let mut turned = turned(&show());
		turned.scenes[0].items[2].format = Some(Format::Portrait);
		turned.validate().unwrap();
		let size = Size {
			width: 1080,
			height: 1920,
		};
		let sized = turned.scenes[0].clone().sized(Some(size));
		assert_eq!(sized.size, Some(size));
		assert_eq!(sized.items.len(), 2, "the portrait-only logo removed");
		assert!(sized.items.iter().all(|item| !item.formats()));
		turned.scenes[0] = sized;
		turned.validate().unwrap();
	}

	/// While the show goes out in both, a scene of overlays given a size of
	/// its own shows one 16:9 picture, so a layer covering the portrait
	/// canvas with its portrait picture is fitted there instead; given the
	/// program's size back, its 9:16 picture is fitted in the box it was
	/// placed in. Each is one edit with the resize.
	#[test]
	fn resizing_a_scene_fits_the_layers_showing_it_in_portrait() {
		let mut show = show();
		show.sources
			.insert("overlays".into(), Source::Scene { scene: "Guests".into() });
		show.scenes[0]
			.items
			.push(Item::new("overlays", Rect::fill(&show.canvas)));
		let turned = turned(&show);
		let whole = Rect::fill(&show.canvas.turned());
		assert_eq!(turned.scenes[0].items[3].portrait.unwrap().rect, whole);
		let resize = |collection: &Collection, size: Option<Size>| {
			let mut draft = collection.draft("Size");
			draft.set_size(1, size).unwrap();
			let edit = draft.finish().unwrap();
			let mut resized = collection.clone();
			resized.apply(&edit).unwrap();
			resized.validate().unwrap();
			resized
		};
		let own = resize(
			&turned,
			Some(Size {
				width: 1280,
				height: 720,
			}),
		);
		let banner = Rect {
			x: 0.0,
			y: 656.0,
			width: 1080.0,
			height: 608.0,
		};
		let overlays = own.scenes[0].items[3].portrait.unwrap();
		assert_eq!(overlays.rect, banner, "its 16:9 picture fitted unstretched");
		assert_eq!(
			own.scenes[0].items[0], turned.scenes[0].items[0],
			"the game is left alone"
		);

		// Given a 9:16 size of its own, its picture keeps its shape there.
		let tall = resize(
			&turned,
			Some(Size {
				width: 1080,
				height: 1920,
			}),
		);
		assert_eq!(tall.scenes[0].items[3].portrait.unwrap().rect, whole);

		let back = resize(&own, None);
		assert_eq!(
			back.scenes[0].items[3].portrait.unwrap().rect,
			Rect {
				x: 369.0,
				y: 656.0,
				width: 342.0,
				height: 608.0
			},
			"its 9:16 picture fitted in the box it was placed in"
		);

		// A place shrunk to nothing has no shape to keep, so it stays as is.
		for (width, height) in [(0.0, 608.0), (1080.0, 0.0), (0.0, 0.0)] {
			let mut empty = turned.clone();
			let placement = empty.scenes[0].items[3].portrait.as_mut().unwrap();
			placement.rect.width = width;
			placement.rect.height = height;
			let rect = placement.rect;
			let own = resize(
				&empty,
				Some(Size {
					width: 1280,
					height: 720,
				}),
			);
			assert_eq!(own.scenes[0].items[3].portrait.unwrap().rect, rect);
		}
	}

	/// A web page covering the canvas in front of the game is mostly
	/// see-through, so the game stays the main picture and the page is
	/// fitted whole rather than cropped to the main picture's band.
	#[test]
	fn a_page_covering_the_canvas_is_not_the_main_picture() {
		let mut show = show();
		let page = Source::Browser {
			url: "https://example.com".into(),
			width: 1920,
			height: 1080,
		};
		show.sources.insert("page".into(), page);
		show.scenes[0].items.push(Item::new("page", Rect::fill(&show.canvas)));
		show.validate().unwrap();
		let turned = turned(&show);
		let items = &turned.scenes[0].items;
		assert_eq!(
			items[0].portrait.unwrap().rect.height,
			1152.0,
			"the game is the main picture"
		);
		assert_eq!(items[3].portrait, None, "the page is fitted");
	}

	#[test]
	fn round_trips_through_the_scene_file() {
		let mut turned = turned(&show());
		turned.scenes[0].items[2].format = Some(Format::Landscape);
		turned.scenes[0].items[2].portrait = None;
		let text = turned.to_toml().unwrap();
		assert!(text.contains("portrait = true"), "{text}");
		assert!(text.contains("format = \"landscape\""), "{text}");
		assert_eq!(Collection::from_toml(&text).unwrap(), turned);
	}
}
