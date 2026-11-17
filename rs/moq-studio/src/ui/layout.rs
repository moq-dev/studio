//! Where each part of the window goes: one pure function of the window's size
//! and the picture's shape, so it is tested without a window.
//!
//! The picture is larger than any other part of the window but the title bar,
//! and as large as the window allows; the scenes, the panic key, and the sound
//! take whichever side is left over, in one of the mock-ups' three
//! arrangements (`quest/m0/ux/mock/shell.html`): `live`, `portrait`, and
//! `compact`. Each part gives way in a small window, so the picture stays the
//! largest of them.

use egui::{Rect, Vec2, pos2, vec2};

/// The smallest window, at which every part still has room and the picture
/// is still the largest, whatever its shape.
pub const MIN: Vec2 = vec2(640.0, 600.0);

/// The title bar's height, and its height when the window stacks.
const TITLE: f32 = 48.0;
const TITLE_STACKED: f32 = 44.0;

/// The strip under the picture in the wide arrangement, and its panic and
/// sound slots' widths.
const STRIP: f32 = 150.0;
const PANIC: f32 = 112.0;
const SOUND: f32 = 360.0;

/// The tall arrangement's columns: the pages, the scenes, two pads wide at
/// most and one at least, and the sound with the panic key under it. The
/// last two narrow with the window.
const SIDEBAR: f32 = 64.0;
const SCENES_COLUMN: [f32; 2] = [120.0, 212.0];
const SOUND_COLUMN: [f32; 2] = [200.0, 292.0];
/// The panic key and the buttons beside it, under the tall arrangement's sound.
const PANIC_TALL: f32 = 96.0;

/// The stacked arrangement's row of scenes and the panic key.
const SCENES_ROW: f32 = 118.0;

/// A sound's row, the room around the rows, and the most rows a stacked
/// window gives room to before they scroll, fewer when the picture needs it.
pub const SOUND_ROW: f32 = 29.0;
const SOUND_PAD: f32 = 20.0;
const SOUND_ROWS: usize = 4;

/// The narrowest the scenes get beside the panic key and the sound in the
/// wide arrangement: two pads and the one that adds a scene.
const SCENES_MIN: f32 = 2.0 * 124.0 + 10.0 + 52.0 + 32.0;

/// Room around the picture for its tally frame, its glow, and its tag.
const FRAME: f32 = 20.0;

/// The narrowest and shortest the stage gets before an arrangement is out.
const STAGE_MIN: f32 = 120.0;

/// How the window is arranged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrangement {
	/// The pages in the title bar, and the scenes, the panic key, and the
	/// sound in one strip under the picture: a landscape show.
	Wide,
	/// The pages in a sidebar, the scenes in a column beside it, then the
	/// picture, then the sound over the panic key: a portrait show.
	Tall,
	/// The picture, then the scenes beside the panic key, then the sound,
	/// each as wide as the window: a window snapped to half a screen.
	Stacked,
}

impl Arrangement {
	/// Every arrangement, the one a tie goes to first.
	const ALL: [Self; 3] = [Self::Wide, Self::Tall, Self::Stacked];
}

/// Where each part of the window goes, in points from its top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
	pub arrangement: Arrangement,
	pub title: Rect,
	/// The pages' sidebar, or `None` while they sit in the title bar.
	pub sidebar: Option<Rect>,
	/// What holds the picture, on the stage's wash.
	pub stage: Rect,
	/// The picture itself, as large as the stage holds at its shape, with
	/// room around it for its frame.
	pub picture: Rect,
	pub scenes: Rect,
	pub panic: Rect,
	pub sound: Rect,
}

impl Layout {
	/// The arrangement of a window `window` large, at least [`MIN`], showing
	/// a picture of `shape` (width over height) and `sounds` sounds, that
	/// gives the picture the most room while it is the largest part, or the
	/// most room at all if none keeps it the largest.
	pub fn new(window: Vec2, shape: f32, sounds: usize) -> Self {
		let (window, shape) = clean(window, shape);
		let mut best: Option<Self> = None;
		for arrangement in Arrangement::ALL {
			let Some(layout) = Self::arrange(arrangement, window, shape, sounds) else {
				continue;
			};
			// Strictly better, so a tie keeps the earlier arrangement.
			let better = |best: Self| (layout.largest(), area(layout.picture)) > (best.largest(), area(best.picture));
			if best.is_none_or(better) {
				best = Some(layout);
			}
		}
		best.expect("the stacked arrangement fits any window of at least MIN")
	}

	/// As [`new`](Self::new), with the sound's slot holding a live picture
	/// rather than a row per sound, as on MIX: the same arrangement, so the
	/// window keeps its shape between pages, with a stacked slot as tall as
	/// the most rows.
	pub fn monitor(window: Vec2, shape: f32, sounds: usize) -> Self {
		let layout = Self::new(window, shape, sounds);
		if layout.arrangement != Arrangement::Stacked {
			return layout;
		}
		let (window, shape) = clean(window, shape);
		Self::stacked(window, shape, SOUND_ROWS).expect("the most rows fit any window of at least MIN")
	}

	/// The parts the picture shares the window with, beside the title bar,
	/// which spans the window's width whatever the picture's shape.
	pub fn parts(&self) -> impl Iterator<Item = Rect> {
		[self.scenes, self.panic, self.sound].into_iter().chain(self.sidebar)
	}

	/// Whether the picture is larger than every other part.
	fn largest(&self) -> bool {
		self.parts().all(|part| area(self.picture) >= area(part))
	}

	/// Every part moved `by`, such as to where the window's contents start.
	pub fn translate(self, by: Vec2) -> Self {
		Self {
			title: self.title.translate(by),
			sidebar: self.sidebar.map(|sidebar| sidebar.translate(by)),
			stage: self.stage.translate(by),
			picture: self.picture.translate(by),
			scenes: self.scenes.translate(by),
			panic: self.panic.translate(by),
			sound: self.sound.translate(by),
			..self
		}
	}

	/// `arrangement` in `window`, or `None` if it leaves a part too little room.
	fn arrange(arrangement: Arrangement, window: Vec2, shape: f32, sounds: usize) -> Option<Self> {
		let at = |x: f32, y: f32, width: f32, height: f32| Rect::from_min_size(pos2(x, y), vec2(width, height));
		let (w, h) = (window.x, window.y);
		let layout = match arrangement {
			Arrangement::Wide => {
				let scenes = w - PANIC - SOUND;
				let stage = h - TITLE - STRIP;
				if scenes < SCENES_MIN || stage < STAGE_MIN {
					return None;
				}
				let strip = h - STRIP;
				Self {
					arrangement,
					title: at(0.0, 0.0, w, TITLE),
					sidebar: None,
					stage: at(0.0, TITLE, w, stage),
					picture: Rect::NOTHING,
					scenes: at(0.0, strip, scenes, STRIP),
					panic: at(scenes, strip, PANIC, STRIP),
					sound: at(scenes + PANIC, strip, SOUND, STRIP),
				}
			}
			Arrangement::Tall => {
				let [least, most] = SCENES_COLUMN;
				let scenes = (w * 0.15).min(h * 0.3).clamp(least, most).round();
				let [least, most] = SOUND_COLUMN;
				let sound = (w * 0.205).min(h * 0.34).clamp(least, most).round();
				let stage = w - SIDEBAR - scenes - sound;
				let body = h - TITLE;
				if stage < STAGE_MIN || body - PANIC_TALL < STAGE_MIN {
					return None;
				}
				let right = SIDEBAR + scenes + stage;
				Self {
					arrangement,
					title: at(0.0, 0.0, w, TITLE),
					sidebar: Some(at(0.0, TITLE, SIDEBAR, body)),
					stage: at(SIDEBAR + scenes, TITLE, stage, body),
					picture: Rect::NOTHING,
					scenes: at(SIDEBAR, TITLE, scenes, body),
					panic: at(right, h - PANIC_TALL, sound, PANIC_TALL),
					sound: at(right, TITLE, sound, body - PANIC_TALL),
				}
			}
			Arrangement::Stacked => {
				// As many rows as the sounds want, fewer while that leaves the
				// picture smaller than another part: they scroll instead.
				let mut layout = None;
				for rows in (1..=sounds.clamp(1, SOUND_ROWS)).rev() {
					let Some(stacked) = Self::stacked(window, shape, rows) else {
						continue;
					};
					layout = Some(stacked);
					if stacked.largest() {
						break;
					}
				}
				layout?
			}
		};
		Some(Self {
			picture: fit(layout.stage.shrink(FRAME), shape),
			..layout
		})
	}

	/// The stacked arrangement in `window` with `rows` rows of sound, or
	/// `None` if that leaves the stage too little room.
	fn stacked(window: Vec2, shape: f32, rows: usize) -> Option<Self> {
		let at = |x: f32, y: f32, width: f32, height: f32| Rect::from_min_size(pos2(x, y), vec2(width, height));
		let w = window.x;
		let sound = rows as f32 * SOUND_ROW + SOUND_PAD;
		let stage = window.y - TITLE_STACKED - SCENES_ROW - sound;
		if stage < STAGE_MIN {
			return None;
		}
		let row = TITLE_STACKED + stage;
		let stage = at(0.0, TITLE_STACKED, w, stage);
		Some(Self {
			arrangement: Arrangement::Stacked,
			title: at(0.0, 0.0, w, TITLE_STACKED),
			sidebar: None,
			stage,
			picture: fit(stage.shrink(FRAME), shape),
			scenes: at(0.0, row, w - PANIC, SCENES_ROW),
			panic: at(w - PANIC, row, PANIC, SCENES_ROW),
			sound: at(0.0, row + SCENES_ROW, w, sound),
		})
	}
}

/// `window` at least [`MIN`], and `shape` a picture's, landscape if it is
/// none.
fn clean(window: Vec2, shape: f32) -> (Vec2, f32) {
	let shape = if shape.is_finite() && shape > 0.0 {
		shape
	} else {
		16.0 / 9.0
	};
	(window.max(MIN), shape)
}

/// The largest rectangle of `shape` (width over height) centered in `rect`.
pub fn fit(rect: Rect, shape: f32) -> Rect {
	let mut size = vec2(rect.width(), rect.width() / shape);
	if size.y > rect.height() {
		size = vec2(rect.height() * shape, rect.height());
	}
	Rect::from_center_size(rect.center(), size.max(Vec2::ZERO))
}

/// The landscape and portrait pictures of a show going out in both, of
/// `shapes` (width over height), side by side `gap` apart: as tall as fits
/// in `rect`, and centered in it.
pub fn pair(rect: Rect, shapes: [f32; 2], gap: f32) -> [Rect; 2] {
	let [landscape, portrait] = shapes;
	let tall = rect.height().min((rect.width() - gap).max(0.0) / (landscape + portrait)).max(0.0);
	let wide = tall * (landscape + portrait) + gap;
	let min = pos2(rect.center().x - wide / 2.0, rect.center().y - tall / 2.0);
	let first = Rect::from_min_size(min, vec2(tall * landscape, tall));
	let second = Rect::from_min_size(pos2(first.right() + gap, min.y), vec2(tall * portrait, tall));
	[first, second]
}

/// The shape, width over height, of the pair [`pair`] places, with a gap
/// `gap` of their height between them.
pub fn pair_shape(shapes: [f32; 2], gap: f32) -> f32 {
	shapes[0] + shapes[1] + gap
}

fn area(rect: Rect) -> f32 {
	rect.width().max(0.0) * rect.height().max(0.0)
}

#[cfg(test)]
mod tests {
	use super::*;

	const LANDSCAPE: f32 = 16.0 / 9.0;
	const PORTRAIT: f32 = 9.0 / 16.0;

	/// The mock-ups' three frames: a landscape show at 1440x900 has the strip
	/// under the picture, a portrait one the columns beside it, and a window
	/// snapped to half the screen stacks.
	#[test]
	fn the_mock_ups_arrangements() {
		let window = vec2(1440.0, 900.0);
		assert_eq!(Layout::new(window, LANDSCAPE, 4).arrangement, Arrangement::Wide);
		let portrait = Layout::new(window, PORTRAIT, 4);
		assert_eq!(portrait.arrangement, Arrangement::Tall);
		assert!(portrait.sidebar.is_some(), "a portrait show's pages are in a sidebar");
		assert_eq!(
			Layout::new(vec2(711.0, 872.0), LANDSCAPE, 4).arrangement,
			Arrangement::Stacked
		);
	}

	/// Both pictures of a show going out in both fit side by side, as tall
	/// as the room allows, with the gap between them.
	#[test]
	fn a_pair_fits_side_by_side() {
		let room = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 400.0));
		let [landscape, portrait] = pair(room, [LANDSCAPE, PORTRAIT], 20.0);
		assert!(room.contains_rect(landscape.union(portrait)));
		assert_eq!(landscape.height(), portrait.height());
		assert!((portrait.left() - landscape.right() - 20.0).abs() < 1e-3);
		assert!((landscape.width() / landscape.height() - LANDSCAPE).abs() < 1e-3);
		assert!((portrait.width() / portrait.height() - PORTRAIT).abs() < 1e-3);
		// Too narrow for the full height, the pair is as wide as the room.
		let narrow = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0));
		let [landscape, portrait] = pair(narrow, [LANDSCAPE, PORTRAIT], 20.0);
		assert!((portrait.right() - landscape.left() - 600.0).abs() < 1e-3);
	}

	/// For every size from the smallest up, of every shape of picture and
	/// count of sounds: every part is inside the window, none overlaps
	/// another, the picture is inside its stage and larger than every part
	/// beside the title bar, and no other arrangement that keeps it so would
	/// give it more room. MIX's layout fits too, with the room of the most
	/// rows for its live picture.
	#[test]
	fn nothing_overlaps_and_the_picture_is_largest() {
		let shapes = [LANDSCAPE, PORTRAIT, 1.0, 4.0 / 3.0, 3.0 / 4.0, 21.0 / 9.0, 9.0 / 21.0];
		let mut width = MIN.x;
		while width <= 3840.0 {
			let mut height = MIN.y;
			while height <= 2160.0 {
				let window = vec2(width, height);
				let bounds = Rect::from_min_size(egui::Pos2::ZERO, window);
				for shape in shapes {
					for sounds in 0..=6 {
						let layout = Layout::new(window, shape, sounds);
						let case = format!("{window:?} {shape} {sounds}: {layout:?}");
						fits(&layout, bounds, &case);
						// MIX keeps the arrangement, and its live picture gets
						// the room of the most rows.
						let monitor = Layout::monitor(window, shape, sounds);
						let mix = format!("MIX, {case}: {monitor:?}");
						fits(&monitor, bounds, &mix);
						assert_eq!(monitor.arrangement, layout.arrangement, "{mix}");
						let rows = SOUND_ROWS as f32 * SOUND_ROW + SOUND_PAD;
						assert!(monitor.sound.height() >= rows, "{mix}");
						assert!(layout.largest(), "the picture is not the largest, {case}");
						for arrangement in Arrangement::ALL {
							let other = Layout::arrange(arrangement, window, shape, sounds);
							if let Some(other) = other.filter(Layout::largest) {
								assert!(
									area(layout.picture) >= area(other.picture),
									"{arrangement:?} is larger, {case}"
								);
							}
						}
					}
				}
				height += 20.0;
			}
			width += 20.0;
		}
	}

	/// A window smaller than the smallest is laid out as the smallest, so
	/// nothing is cut off on a desktop that ignores the minimum.
	#[test]
	fn a_smaller_window_is_laid_out_as_the_smallest() {
		assert_eq!(
			Layout::new(vec2(100.0, 50.0), LANDSCAPE, 2),
			Layout::new(MIN, LANDSCAPE, 2)
		);
	}

	/// Every part of `layout` inside `bounds`, none overlapping another, and
	/// the picture inside its stage.
	fn fits(layout: &Layout, bounds: Rect, case: &str) {
		let parts: Vec<Rect> = [layout.title, layout.stage, layout.scenes, layout.panic, layout.sound]
			.into_iter()
			.chain(layout.sidebar)
			.collect();
		for (index, part) in parts.iter().enumerate() {
			assert!(part.is_positive(), "an empty part, {case}");
			assert!(bounds.contains_rect(*part), "a part outside the window, {case}");
			for other in &parts[index + 1..] {
				assert!(!part.intersects(*other) || touching(*part, *other), "overlap, {case}");
			}
		}
		assert!(layout.stage.contains_rect(layout.picture), "{case}");
	}

	/// Whether `a` and `b` only share an edge.
	fn touching(a: Rect, b: Rect) -> bool {
		let overlap = a.intersect(b);
		overlap.width() <= 0.0 || overlap.height() <= 0.0
	}
}
