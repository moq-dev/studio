//! Slot layouts: where a scene's [`Arrangement`] puts the items in its
//! slots, given which of them take one.

use crate::collection::{Arrangement, Canvas, Collection, Crop, Item, Rect, Slot};

/// The shape of every slot: a camera's, a screen's, and a 1080p canvas's.
const ASPECT: f32 = 16.0 / 9.0;

/// The margin around the slots, as a share of the canvas's shorter side.
const MARGIN: f32 = 0.04;

/// The gap between two slots, as a share of the canvas's shorter side.
const GAP: f32 = 0.02;

/// The widest the column of tiles grows, or the tallest the row, as a share
/// of the room for the slots, so the main item stays the largest.
const SIDE: f32 = 0.4;

/// The widest an inset tile is, as a share of the canvas's width.
const INSET: f32 = 0.2;

/// Where `arrangement` puts a main item, if there is one, and `tiles` tiles,
/// in order, on a `canvas`-sized picture, in whole pixels.
pub(crate) fn places(arrangement: Arrangement, canvas: Canvas, main: bool, tiles: usize) -> (Option<Rect>, Vec<Rect>) {
	let (width, height) = (canvas.width as f32, canvas.height as f32);
	let short = width.min(height);
	let (margin, gap) = ((short * MARGIN).round(), (short * GAP).round());
	let room = Rect {
		x: margin,
		y: margin,
		width: (width - 2.0 * margin).max(0.0),
		height: (height - 2.0 * margin).max(0.0),
	};
	let (main, tiles) = match (main, tiles) {
		(false, tiles) => (None, grid(room, tiles, gap)),
		(true, 0) if arrangement == Arrangement::Inset => (Some(Rect::fill(&canvas)), Vec::new()),
		(true, 0) => (Some(fit(room, ASPECT)), Vec::new()),
		(true, tiles) => match arrangement {
			Arrangement::Column => column(room, tiles, gap),
			Arrangement::Row => row(room, tiles, gap),
			Arrangement::Grid => {
				let mut cells = grid(room, tiles + 1, gap);
				let main = cells.remove(0);
				(Some(main), cells)
			}
			Arrangement::Inset => inset(canvas, room, tiles, margin, gap),
		},
	};
	(main.map(whole), tiles.into_iter().map(whole).collect())
}

/// The main item on the left and the tiles stacked on its right, the pair
/// centered in `room`.
fn column(room: Rect, tiles: usize, gap: f32) -> (Option<Rect>, Vec<Rect>) {
	let count = tiles as f32;
	let tall = (room.height - gap * (count - 1.0)) / count;
	let wide = (tall * ASPECT).min((room.width - gap) * SIDE).max(0.0);
	let tall = wide / ASPECT;
	let main = fit(
		Rect {
			width: (room.width - wide - gap).max(0.0),
			..room
		},
		ASPECT,
	);
	let left = room.x + (room.width - main.width - gap - wide) / 2.0;
	let main = Rect { x: left, ..main };
	let stack = tall * count + gap * (count - 1.0);
	let top = room.y + (room.height - stack) / 2.0;
	let x = left + main.width + gap;
	let tiles = (0..tiles).map(|index| Rect {
		x,
		y: top + index as f32 * (tall + gap),
		width: wide,
		height: tall,
	});
	(Some(main), tiles.collect())
}

/// The main item on top and the tiles in a row under it, the pair centered
/// in `room`.
fn row(room: Rect, tiles: usize, gap: f32) -> (Option<Rect>, Vec<Rect>) {
	let count = tiles as f32;
	let wide = (room.width - gap * (count - 1.0)) / count;
	let tall = (wide / ASPECT).min((room.height - gap) * SIDE).max(0.0);
	let wide = tall * ASPECT;
	let main = fit(
		Rect {
			height: (room.height - tall - gap).max(0.0),
			..room
		},
		ASPECT,
	);
	let top = room.y + (room.height - main.height - gap - tall) / 2.0;
	let main = Rect { y: top, ..main };
	let span = wide * count + gap * (count - 1.0);
	let left = room.x + (room.width - span) / 2.0;
	let y = top + main.height + gap;
	let tiles = (0..tiles).map(|index| Rect {
		x: left + index as f32 * (wide + gap),
		y,
		width: wide,
		height: tall,
	});
	(Some(main), tiles.collect())
}

/// The main item filling the canvas and the tiles stacked up from its
/// bottom-right corner, inside the margin.
fn inset(canvas: Canvas, room: Rect, tiles: usize, margin: f32, gap: f32) -> (Option<Rect>, Vec<Rect>) {
	let count = tiles as f32;
	let fits = (room.height - gap * (count - 1.0)) / count * ASPECT;
	let wide = (canvas.width as f32 * INSET).min(fits).max(0.0);
	let tall = wide / ASPECT;
	let x = canvas.width as f32 - margin - wide;
	let bottom = canvas.height as f32 - margin;
	let tiles = (0..tiles).map(|index| Rect {
		x,
		y: bottom - tall - index as f32 * (tall + gap),
		width: wide,
		height: tall,
	});
	(Some(Rect::fill(&canvas)), tiles.collect())
}

/// `count` equal cells filling as much of `room` as they can, row by row,
/// the last row centered and the grid centered in `room`.
fn grid(room: Rect, count: usize, gap: f32) -> Vec<Rect> {
	if count == 0 {
		return Vec::new();
	}
	// The number of columns that makes the cells largest.
	let cell = |columns: usize| {
		let rows = count.div_ceil(columns);
		let wide = (room.width - gap * (columns as f32 - 1.0)) / columns as f32;
		let tall = (room.height - gap * (rows as f32 - 1.0)) / rows as f32;
		let wide = wide.min(tall * ASPECT).max(0.0);
		(wide, wide / ASPECT)
	};
	let columns = (1..=count)
		.max_by(|a, b| cell(*a).0.total_cmp(&cell(*b).0).then(b.cmp(a)))
		.expect("at least one cell");
	let rows = count.div_ceil(columns);
	let (wide, tall) = cell(columns);
	let block = tall * rows as f32 + gap * (rows as f32 - 1.0);
	let top = room.y + (room.height - block) / 2.0;
	(0..count)
		.map(|index| {
			let (row, column) = (index / columns, index % columns);
			let across = (count - row * columns).min(columns) as f32;
			let span = wide * across + gap * (across - 1.0);
			let left = room.x + (room.width - span) / 2.0;
			Rect {
				x: left + column as f32 * (wide + gap),
				y: top + row as f32 * (tall + gap),
				width: wide,
				height: tall,
			}
		})
		.collect()
}

/// The largest rect of `aspect`, width over height, centered in `room`.
fn fit(room: Rect, aspect: f32) -> Rect {
	let width = room.width.min(room.height * aspect).max(0.0);
	let height = width / aspect;
	Rect {
		x: room.x + (room.width - width) / 2.0,
		y: room.y + (room.height - height) / 2.0,
		width,
		height,
	}
}

/// `rect` with its edges on whole pixels, so a slot draws sharp and two meet
/// without a seam.
fn whole(rect: Rect) -> Rect {
	let (left, top) = (rect.x.round(), rect.y.round());
	Rect {
		x: left,
		y: top,
		width: (rect.x + rect.width).round() - left,
		height: (rect.y + rect.height).round() - top,
	}
}

/// The crop that trims what `crop` leaves of a `size` picture evenly from two
/// opposite sides to the shape of `rect`, so it covers it unstretched.
pub(crate) fn cover(crop: Crop, size: (u32, u32), rect: &Rect) -> Crop {
	let width = size.0 as f32 * (1.0 - crop.left - crop.right);
	let height = size.1 as f32 * (1.0 - crop.top - crop.bottom);
	let (shown, wanted) = (width / height, rect.width / rect.height);
	if !shown.is_finite() || shown <= 0.0 || !wanted.is_finite() || wanted <= 0.0 {
		return Crop::default();
	}
	if shown > wanted {
		let trim = (1.0 - wanted / shown) / 2.0;
		Crop {
			left: trim,
			right: trim,
			..Crop::default()
		}
	} else {
		let trim = (1.0 - shown / wanted) / 2.0;
		Crop {
			top: trim,
			bottom: trim,
			..Crop::default()
		}
	}
}

impl Collection {
	/// The items of the scene called `scene` as they are drawn, in order:
	/// each in a slot placed by the scene's arrangement, its picture cropped
	/// to cover the slot given its size from `size`, and hidden if it takes
	/// no slot. An item takes one while it is visible, its role is filled,
	/// and `present` says its source is there, asked by the source the item
	/// shows through its role. `None` for a scene with no item in a slot, or
	/// no such scene.
	pub fn arranged(
		&self,
		scene: &str,
		present: impl Fn(&str) -> bool,
		size: impl Fn(&str) -> Option<(u32, u32)>,
	) -> Option<Vec<Item>> {
		let scene = self.scene(scene)?;
		if scene.items.iter().all(|item| item.slot.is_none()) {
			return None;
		}
		let takes = |item: &Item| item.visible && !self.unfilled(&item.source) && present(self.resolve(&item.source));
		let main = scene
			.items
			.iter()
			.any(|item| item.slot == Some(Slot::Main) && takes(item));
		let tiles = scene
			.items
			.iter()
			.filter(|item| item.slot == Some(Slot::Tile) && takes(item))
			.count();
		let (main, tiles) = places(scene.arrangement, scene.canvas(self.canvas), main, tiles);
		let mut tiles = tiles.into_iter();
		let arranged = scene.items.iter().map(|item| {
			let Some(slot) = item.slot else {
				return item.clone();
			};
			let rect = match takes(item) {
				true if slot == Slot::Main => main,
				true => tiles.next(),
				false => None,
			};
			let Some(rect) = rect else {
				return Item {
					visible: false,
					..item.clone()
				};
			};
			let crop = match size(self.resolve(&item.source)) {
				Some(size) => item.crop.then(&cover(self.crop(item), size, &rect)),
				None => item.crop,
			};
			Item {
				rect,
				crop,
				..item.clone()
			}
		});
		Some(arranged.collect())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const HD: Canvas = Canvas {
		width: 1920,
		height: 1080,
		framerate: 30,
	};

	fn overlaps(a: &Rect, b: &Rect) -> bool {
		a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
	}

	fn inside(rect: &Rect, canvas: Canvas) -> bool {
		rect.x >= 0.0
			&& rect.y >= 0.0
			&& rect.x + rect.width <= canvas.width as f32
			&& rect.y + rect.height <= canvas.height as f32
	}

	/// Every arrangement, on a wide and a tall canvas, with and without a
	/// main item and with any number of tiles, keeps each slot on the
	/// canvas, apart from the others, and the main item the largest; a
	/// tile overlaps the main item only when it is inset in it.
	#[test]
	fn slots_stay_on_the_canvas_apart_and_the_main_one_largest() {
		let tall = Canvas {
			width: 1080,
			height: 1920,
			framerate: 30,
		};
		for canvas in [HD, tall] {
			for arrangement in Arrangement::ALL {
				for main in [false, true] {
					for count in 0..=9 {
						let (big, tiles) = places(arrangement, canvas, main, count);
						let on = format!("{arrangement:?} on {canvas:?}, main {main}, {count} tiles");
						assert_eq!(big.is_some(), main, "{on}");
						assert_eq!(tiles.len(), count, "{on}");
						for (index, tile) in tiles.iter().enumerate() {
							assert!(inside(tile, canvas), "{on}: {tile:?}");
							assert!(tile.width > 0.0 && tile.height > 0.0, "{on}: {tile:?}");
							for other in &tiles[index + 1..] {
								assert!(!overlaps(tile, other), "{on}: {tile:?} and {other:?}");
							}
							if let Some(big) = big {
								let area = |rect: &Rect| rect.width * rect.height;
								// Equal grid cells may round a pixel apart.
								assert!(area(&big) >= area(tile) * 0.99, "{on}: {big:?} and {tile:?}");
								let covers = arrangement == Arrangement::Inset;
								assert_eq!(overlaps(&big, tile), covers, "{on}: {big:?} and {tile:?}");
							}
						}
						if let Some(big) = big {
							assert!(inside(&big, canvas), "{on}: {big:?}");
						}
					}
				}
			}
		}
	}

	/// The `autolayout` frame of the mock-up: a second guest shrinks the
	/// column and grows the main picture, which keeps its place on the left.
	#[test]
	fn a_second_guest_shrinks_the_column_and_grows_the_main_picture() {
		let (one, column) = places(Arrangement::Column, HD, true, 2);
		let (two, longer) = places(Arrangement::Column, HD, true, 3);
		let (one, two) = (one.unwrap(), two.unwrap());
		assert!(two.width > one.width, "{one:?} then {two:?}");
		assert!(longer[0].width < column[0].width);
		assert!(column.iter().all(|tile| tile.x > one.x + one.width));
		for main in [one, two] {
			assert!((main.width / main.height - ASPECT).abs() < 0.01, "{main:?}");
		}
	}
}
