//! Where an item dragged in the preview lands: the part of it the pointer
//! holds, and how it snaps to the canvas, other items, and a grid.
//!
//! Plain math in canvas pixels with no egui types, so it is tested here; the
//! preview only converts the pointer and draws the result.

use moq_scene::{Canvas, Item, Rect};

/// How close two positions are to count as lined up. Snapping makes them
/// equal up to float rounding; unsnapped drags never draw guides.
const EPSILON: f32 = 0.01;

/// A direction on the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
	X,
	Y,
}

impl Axis {
	pub const ALL: [Self; 2] = [Self::X, Self::Y];

	fn other(self) -> Self {
		match self {
			Self::X => Self::Y,
			Self::Y => Self::X,
		}
	}

	/// This axis's coordinate of `point`.
	fn of(self, point: [f32; 2]) -> f32 {
		match self {
			Self::X => point[0],
			Self::Y => point[1],
		}
	}

	/// Where `rect` starts and ends along this axis.
	fn span(self, rect: &Rect) -> (f32, f32) {
		match self {
			Self::X => (rect.x, rect.x + rect.width),
			Self::Y => (rect.y, rect.y + rect.height),
		}
	}

	fn set_span(self, rect: &mut Rect, (start, end): (f32, f32)) {
		match self {
			Self::X => (rect.x, rect.width) = (start, end - start),
			Self::Y => (rect.y, rect.height) = (start, end - start),
		}
	}

	fn length(self, canvas: &Canvas) -> f32 {
		match self {
			Self::X => canvas.width as f32,
			Self::Y => canvas.height as f32,
		}
	}
}

/// One edge of an item along an axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
	Start,
	End,
}

/// The part of an item a drag holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grab {
	/// The whole item, which moves.
	Move,
	/// An edge or corner, which resizes: the edge held along each axis.
	Resize { x: Option<Side>, y: Option<Side> },
}

/// What a grab moves along one axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Moves {
	Both,
	Edge(Side),
	Neither,
}

impl Grab {
	/// What a press at `point` grabs of `rect`: an edge or corner within
	/// `reach` resizes, and the rest of the inside moves. Inside, an edge
	/// reaches at most a third of the way across, so a small item still moves.
	pub fn at(rect: &Rect, point: [f32; 2], reach: f32) -> Option<Self> {
		let mut sides = [None; 2];
		for (axis, side) in Axis::ALL.into_iter().zip(&mut sides) {
			let (start, end) = axis.span(rect);
			let p = axis.of(point);
			if p < start - reach || p > end + reach {
				return None;
			}
			let inward = reach.min((end - start) / 3.0);
			// How far inside each edge the point is, negative outside it.
			let (from_start, from_end) = (p - start, end - p);
			*side = match (from_start <= inward, from_end <= inward) {
				(true, true) if from_start <= from_end => Some(Side::Start),
				(true, true) => Some(Side::End),
				(true, false) => Some(Side::Start),
				(false, true) => Some(Side::End),
				(false, false) => None,
			};
		}
		Some(match sides {
			[None, None] => Self::Move,
			[x, y] => Self::Resize { x, y },
		})
	}

	fn moves(self, axis: Axis) -> Moves {
		let side = match (self, axis) {
			(Self::Move, _) => return Moves::Both,
			(Self::Resize { x, .. }, Axis::X) => x,
			(Self::Resize { y, .. }, Axis::Y) => y,
		};
		side.map_or(Moves::Neither, Moves::Edge)
	}

	/// `rect` with what this grab holds dragged by `delta`. A resized edge
	/// stops at the opposite one rather than turning the item inside out.
	///
	/// With an `aspect`, the width over height, a resize keeps that shape: the
	/// held edge along one axis leads, as the one held alone or, at a corner,
	/// the one dragged further for its size, and the other axis follows.
	pub fn drag(self, rect: Rect, delta: [f32; 2], aspect: Option<f32>) -> Rect {
		let mut dragged = rect;
		for axis in Axis::ALL {
			shift(&mut dragged, axis, self.moves(axis), axis.of(delta));
		}
		let lead = match self {
			Self::Move | Self::Resize { x: None, y: None } => return dragged,
			Self::Resize { x: Some(_), y: None } => Axis::X,
			Self::Resize { x: None, y: Some(_) } => Axis::Y,
			Self::Resize { .. } => {
				// How far each axis grew or shrank, for its size.
				let change = |axis: Axis| {
					let (start, end) = axis.span(&rect);
					let (from, to) = axis.span(&dragged);
					((to - from) - (end - start)).abs() / (end - start).max(1.0)
				};
				if change(Axis::X) >= change(Axis::Y) {
					Axis::X
				} else {
					Axis::Y
				}
			}
		};
		match aspect {
			Some(aspect) => self.keep_aspect(dragged, aspect, lead),
			None => dragged,
		}
	}

	/// `rect` reshaped to `aspect`, the width over height, keeping its span
	/// along `lead`. The other axis moves the edge this grab holds on it, or
	/// stays centered where it holds none.
	fn keep_aspect(self, rect: Rect, aspect: f32, lead: Axis) -> Rect {
		let mut rect = rect;
		let (start, end) = lead.span(&rect);
		let follow = lead.other();
		let length = match lead {
			Axis::X => (end - start) / aspect,
			Axis::Y => (end - start) * aspect,
		};
		let (from, to) = follow.span(&rect);
		let span = match self.moves(follow) {
			Moves::Edge(Side::Start) => (to - length, to),
			Moves::Edge(Side::End) => (from, from + length),
			Moves::Both | Moves::Neither => {
				let center = (from + to) / 2.0;
				(center - length / 2.0, center + length / 2.0)
			}
		};
		follow.set_span(&mut rect, span);
		rect
	}
}

/// The width over height of `rect`, if it has a shape to keep.
pub fn aspect(rect: &Rect) -> Option<f32> {
	let aspect = rect.width / rect.height;
	(aspect.is_finite() && aspect > 0.0).then_some(aspect)
}

/// Move what `moves` names of `rect` along `axis` by `by`.
fn shift(rect: &mut Rect, axis: Axis, moves: Moves, by: f32) {
	let (start, end) = axis.span(rect);
	let span = match moves {
		Moves::Both => (start + by, end + by),
		Moves::Edge(Side::Start) => ((start + by).min(end), end),
		Moves::Edge(Side::End) => (start, (end + by).max(start)),
		Moves::Neither => return,
	};
	axis.set_span(rect, span);
}

/// The item of `items` a press at `point` grabs, and what part of it. The
/// `selected` item's edges come first, since they reach past it; then the
/// frontmost item under the point, so a selected backdrop does not hide what
/// is in front of it. Hidden items are not shown, so nothing grabs them.
pub fn hit(items: &[Item], selected: Option<usize>, point: [f32; 2], reach: f32) -> Option<(usize, Grab)> {
	if let Some(index) = selected
		&& let Some(item) = items.get(index).filter(|item| item.visible)
		&& let Some(grab @ Grab::Resize { .. }) = Grab::at(&item.rect, point, reach)
	{
		return Some((index, grab));
	}
	let inside = |rect: &Rect| {
		Axis::ALL.into_iter().all(|axis| {
			let (start, end) = axis.span(rect);
			(start..=end).contains(&axis.of(point))
		})
	};
	let index = (0..items.len())
		.rev()
		.find(|&index| items[index].visible && inside(&items[index].rect))?;
	Some((index, Grab::Move))
}

/// How a drag snaps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
	/// How far, in canvas pixels, an edge or center is pulled to a line it nears.
	pub reach: f32,
	/// The spacing of a grid to snap to, in canvas pixels, if any.
	pub grid: Option<f32>,
	/// The width over height a resize keeps, if any, of the item at rest.
	pub aspect: Option<f32>,
	/// How far keyframes have the item from where it rests, added to each
	/// field of its rect. It snaps where it shows, but keeps its shape at rest.
	pub posed: Rect,
}

/// A dragged item once snapped, and the hints to draw while it is held.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapped {
	pub rect: Rect,
	pub guides: Vec<Guide>,
	pub gaps: Vec<Gap>,
}

/// A line where the dragged item lines up with the canvas or another item.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
	/// The axis the line marks a position on: an `X` guide is vertical.
	pub axis: Axis,
	/// The position on `axis`.
	pub at: f32,
	/// Where the line starts and ends on the other axis.
	pub from: f32,
	pub to: f32,
}

/// The empty space along `axis` between two items, drawn when the dragged
/// item's space to a neighbor matches another.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gap {
	pub axis: Axis,
	pub start: f32,
	pub end: f32,
	/// Where on the other axis to draw it.
	pub at: f32,
}

impl Snap {
	/// Snap `rect`, the item at rest once `grab` has dragged it, to the
	/// canvas, the `others`, and the grid, where it shows. Along each axis,
	/// the nearest line within reach of a moving edge or center wins, as does
	/// the spacing of a neighboring pair when the whole item moves.
	///
	/// A resize keeping an aspect snaps only the axis whose snap moves the
	/// item least, and the other follows its shape, as long as no edge moves
	/// out of reach. The snapped rect is at rest.
	pub fn apply(&self, grab: Grab, rect: Rect, others: &[Rect], canvas: &Canvas) -> Snapped {
		// An edge shown is the edge at rest plus a constant, so a shift
		// measured where it shows moves it as far at rest.
		let shown = |rect: &Rect| Rect {
			x: rect.x + self.posed.x,
			y: rect.y + self.posed.y,
			width: rect.width + self.posed.width,
			height: rect.height + self.posed.height,
		};
		let mut rect = rect;
		match self.aspect.filter(|_| grab != Grab::Move) {
			None => {
				for axis in Axis::ALL {
					if let Some(best) = self.nearest(grab, axis, &shown(&rect), others, canvas) {
						shift(&mut rect, axis, grab.moves(axis), best);
					}
				}
			}
			Some(aspect) => {
				// How far a snap moves any edge, the one that follows the
				// shape included.
				let moved = |snapped: &Rect| {
					Axis::ALL
						.into_iter()
						.map(|axis| {
							let ((start, end), (from, to)) = (axis.span(&rect), axis.span(snapped));
							(from - start).abs().max((to - end).abs())
						})
						.fold(0.0, f32::max)
				};
				let best = Axis::ALL
					.into_iter()
					.filter_map(|axis| {
						let mut snapped = rect;
						shift(
							&mut snapped,
							axis,
							grab.moves(axis),
							self.nearest(grab, axis, &shown(&rect), others, canvas)?,
						);
						let snapped = grab.keep_aspect(snapped, aspect, axis);
						let moved = moved(&snapped);
						(moved <= self.reach).then_some((moved, snapped))
					})
					.min_by(|(a, _), (b, _)| a.total_cmp(b));
				if let Some((_, snapped)) = best {
					rect = snapped;
				}
			}
		}

		let mut guides = Vec::new();
		let mut gaps = Vec::new();
		let showing = shown(&rect);
		for axis in Axis::ALL {
			let moves = grab.moves(axis);
			guides.extend(lined_up(
				axis,
				&showing,
				&points(axis.span(&showing), moves),
				others,
				canvas,
			));
			if moves == Moves::Both {
				gaps.extend(equal_gaps(axis, &showing, others));
			}
		}
		Snapped { rect, guides, gaps }
	}

	/// The shortest shift along `axis`, within reach, that snaps what `grab`
	/// moves of `rect`.
	fn nearest(&self, grab: Grab, axis: Axis, rect: &Rect, others: &[Rect], canvas: &Canvas) -> Option<f32> {
		let moves = grab.moves(axis);
		let points = points(axis.span(rect), moves);
		let mut shifts: Vec<f32> = Vec::new();
		for line in lines(axis, others, canvas) {
			shifts.extend(points.iter().map(|point| line - point));
		}
		if let Some(grid) = self.grid {
			shifts.extend(points.iter().map(|point| (point / grid).round() * grid - point));
		}
		// Spacing places the whole item, so a resize never takes it. An
		// alignment wins a tie, since it comes first.
		if moves == Moves::Both {
			shifts.extend(spacings(axis, rect, others));
		}
		shifts
			.into_iter()
			.filter(|shift| shift.abs() <= self.reach)
			.min_by(|a, b| a.abs().total_cmp(&b.abs()))
	}
}

/// The positions along an axis that `moves` carries: every edge and the
/// center when the whole item moves, or the one edge a resize holds.
fn points((start, end): (f32, f32), moves: Moves) -> Vec<f32> {
	match moves {
		Moves::Both => vec![start, (start + end) / 2.0, end],
		Moves::Edge(Side::Start) => vec![start],
		Moves::Edge(Side::End) => vec![end],
		Moves::Neither => Vec::new(),
	}
}

/// The canvas's edges and center along `axis`.
fn canvas_lines(axis: Axis, canvas: &Canvas) -> [f32; 3] {
	let length = axis.length(canvas);
	[0.0, length / 2.0, length]
}

/// An item's edges and center along `axis`.
fn rect_lines(axis: Axis, rect: &Rect) -> [f32; 3] {
	let (start, end) = axis.span(rect);
	[start, (start + end) / 2.0, end]
}

/// Every line along `axis` a moving edge or center snaps to.
fn lines(axis: Axis, others: &[Rect], canvas: &Canvas) -> Vec<f32> {
	let others = others.iter().flat_map(|other| rect_lines(axis, other));
	canvas_lines(axis, canvas).into_iter().chain(others).collect()
}

/// A guide for every line along `axis` that one of `points` of `rect` sits
/// on, spanning the item and what it lines up with. Guides at the same
/// position merge.
fn lined_up(axis: Axis, rect: &Rect, points: &[f32], others: &[Rect], canvas: &Canvas) -> Vec<Guide> {
	let across = axis.other();
	let (from, to) = across.span(rect);
	let mut guides: Vec<Guide> = Vec::new();
	let mut add = |at: f32, (other_from, other_to): (f32, f32)| {
		if !points.iter().any(|point| (point - at).abs() < EPSILON) {
			return;
		}
		let (from, to) = (from.min(other_from), to.max(other_to));
		match guides.iter_mut().find(|guide| (guide.at - at).abs() < EPSILON) {
			Some(guide) => (guide.from, guide.to) = (guide.from.min(from), guide.to.max(to)),
			None => guides.push(Guide { axis, at, from, to }),
		}
	};
	for at in canvas_lines(axis, canvas) {
		add(at, (0.0, across.length(canvas)));
	}
	for other in others {
		for at in rect_lines(axis, other) {
			add(at, across.span(other));
		}
	}
	guides
}

/// The `others` beside `rect` along `axis`: those overlapping it across.
fn neighbors(axis: Axis, rect: &Rect, others: &[Rect]) -> Vec<Rect> {
	let (from, to) = axis.other().span(rect);
	others
		.iter()
		.filter(|other| {
			let (other_from, other_to) = axis.other().span(other);
			other_from < to && other_to > from
		})
		.copied()
		.collect()
}

/// The empty spaces along `axis` between consecutive `rects`, by start.
fn gaps(axis: Axis, mut rects: Vec<Rect>) -> Vec<Gap> {
	rects.sort_by(|a, b| axis.span(a).0.total_cmp(&axis.span(b).0));
	rects
		.windows(2)
		.filter_map(|pair| {
			let (_, start) = axis.span(&pair[0]);
			let (end, _) = axis.span(&pair[1]);
			let (a, b) = (axis.other().span(&pair[0]), axis.other().span(&pair[1]));
			// The middle of where they overlap across, or of the space between.
			let at = (a.0.max(b.0) + a.1.min(b.1)) / 2.0;
			(end - start > EPSILON).then_some(Gap { axis, start, end, at })
		})
		.collect()
}

/// The shifts along `axis` that space `rect` evenly with its neighbors:
/// centered between two, or as far past a pair as they are apart.
fn spacings(axis: Axis, rect: &Rect, others: &[Rect]) -> Vec<f32> {
	let (start, end) = axis.span(rect);
	let size = end - start;
	let mut row = neighbors(axis, rect, others);
	row.sort_by(|a, b| axis.span(a).0.total_cmp(&axis.span(b).0));
	let mut targets = Vec::new();
	for pair in row.windows(2) {
		let ((first, first_end), (second, second_end)) = (axis.span(&pair[0]), axis.span(&pair[1]));
		let space = second - first_end;
		if space <= EPSILON {
			continue;
		}
		if space >= size {
			targets.push(first_end + (space - size) / 2.0);
		}
		targets.push(second_end + space);
		targets.push(first - space - size);
	}
	targets
		.into_iter()
		.map(|target| target - start)
		.filter(|&by| {
			// Only a spacing the hints show, which an item in the way breaks.
			let mut moved = *rect;
			shift(&mut moved, axis, Moves::Both, by);
			!equal_gaps(axis, &moved, others).is_empty()
		})
		.collect()
}

/// The spaces along `axis` beside `rect` that match another space between
/// its neighbors or on its other side, with the spaces they match.
fn equal_gaps(axis: Axis, rect: &Rect, others: &[Rect]) -> Vec<Gap> {
	let mut row = neighbors(axis, rect, others);
	row.push(*rect);
	let all = gaps(axis, row);
	let (start, end) = axis.span(rect);
	let mut shown: Vec<Gap> = Vec::new();
	for own in all.iter().filter(|gap| gap.end == start || gap.start == end) {
		let length = own.end - own.start;
		let matches: Vec<&Gap> = all
			.iter()
			.filter(|gap| *gap != own && ((gap.end - gap.start) - length).abs() < EPSILON)
			.collect();
		if matches.is_empty() {
			continue;
		}
		for gap in std::iter::once(own).chain(matches) {
			if !shown.contains(gap) {
				shown.push(*gap);
			}
		}
	}
	shown
}

#[cfg(test)]
mod tests {
	use super::*;

	const CANVAS: Canvas = Canvas {
		width: 1920,
		height: 1080,
		framerate: 30,
	};

	const fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
		Rect { x, y, width, height }
	}

	const SNAP: Snap = Snap {
		reach: 10.0,
		grid: None,
		aspect: None,
		posed: rect(0.0, 0.0, 0.0, 0.0),
	};

	#[test]
	fn grabs_edges_corners_and_inside() {
		let item = rect(100.0, 100.0, 200.0, 100.0);
		assert_eq!(Grab::at(&item, [200.0, 150.0], 8.0), Some(Grab::Move));
		assert_eq!(Grab::at(&item, [50.0, 150.0], 8.0), None);
		// Just outside an edge still holds it.
		let right = Grab::Resize {
			x: Some(Side::End),
			y: None,
		};
		assert_eq!(Grab::at(&item, [305.0, 150.0], 8.0), Some(right));
		let corner = Grab::Resize {
			x: Some(Side::Start),
			y: Some(Side::Start),
		};
		assert_eq!(Grab::at(&item, [96.0, 97.0], 8.0), Some(corner));
	}

	#[test]
	fn a_small_item_still_moves() {
		let item = rect(100.0, 100.0, 12.0, 12.0);
		assert_eq!(Grab::at(&item, [106.0, 106.0], 8.0), Some(Grab::Move));
		let left = Grab::Resize {
			x: Some(Side::Start),
			y: None,
		};
		assert_eq!(Grab::at(&item, [101.0, 106.0], 8.0), Some(left));
	}

	#[test]
	fn a_resize_stops_at_the_opposite_edge() {
		let item = rect(100.0, 100.0, 200.0, 100.0);
		let left = Grab::Resize {
			x: Some(Side::Start),
			y: None,
		};
		assert_eq!(left.drag(item, [500.0, 40.0], None), rect(300.0, 100.0, 0.0, 100.0));
		let bottom_right = Grab::Resize {
			x: Some(Side::End),
			y: Some(Side::End),
		};
		assert_eq!(
			bottom_right.drag(item, [10.0, -20.0], None),
			rect(100.0, 100.0, 210.0, 80.0)
		);
	}

	#[test]
	fn a_locked_resize_keeps_the_shape() {
		let item = rect(100.0, 100.0, 200.0, 100.0);
		let shape = aspect(&item);
		// A corner follows the axis dragged further for its size, here the
		// width, and the height grows from the top edge it does not hold.
		let bottom_right = Grab::Resize {
			x: Some(Side::End),
			y: Some(Side::End),
		};
		assert_eq!(
			bottom_right.drag(item, [100.0, 10.0], shape),
			rect(100.0, 100.0, 300.0, 150.0)
		);
		let top_left = Grab::Resize {
			x: Some(Side::Start),
			y: Some(Side::Start),
		};
		assert_eq!(
			top_left.drag(item, [10.0, 40.0], shape),
			rect(180.0, 140.0, 120.0, 60.0)
		);
		// An edge alone grows the other axis about its center.
		let right = Grab::Resize {
			x: Some(Side::End),
			y: None,
		};
		assert_eq!(right.drag(item, [50.0, 30.0], shape), rect(100.0, 87.5, 250.0, 125.0));
		// Moving keeps the shape anyway.
		assert_eq!(
			Grab::Move.drag(item, [5.0, 5.0], shape),
			rect(105.0, 105.0, 200.0, 100.0)
		);
		assert_eq!(aspect(&rect(0.0, 0.0, 10.0, 0.0)), None);
	}

	#[test]
	fn a_locked_resize_snaps_one_axis_and_keeps_the_shape() {
		let snap = Snap {
			aspect: Some(2.0),
			..SNAP
		};
		let bottom_right = Grab::Resize {
			x: Some(Side::End),
			y: Some(Side::End),
		};
		// The right edge at 396 is 4 px from the other item's left edge, and
		// the bottom at 247 is 3 px from its top, but snapping the bottom
		// would move the right edge 6 px, so the right edge's snap wins.
		let others = [rect(400.0, 250.0, 100.0, 50.0)];
		let snapped = snap.apply(bottom_right, rect(100.0, 100.0, 296.0, 147.0), &others, &CANVAS);
		assert_eq!(snapped.rect, rect(100.0, 100.0, 300.0, 150.0));
		assert_eq!(snapped.guides.len(), 2, "both edges land on the other item's");
		// With nothing in reach it stays as dragged.
		let item = rect(100.0, 100.0, 300.0, 150.0);
		assert_eq!(snap.apply(bottom_right, item, &[], &CANVAS).rect, item);
	}

	#[test]
	fn a_locked_snap_moves_no_edge_out_of_reach() {
		let snap = Snap {
			aspect: Some(100.0),
			..SNAP
		};
		let bottom_right = Grab::Resize {
			x: Some(Side::End),
			y: Some(Side::End),
		};
		// A thin strip's bottom is 5 px from the other item's top, and its right
		// edge 8 px from its left. Snapping the bottom would widen it 500 px.
		let strip = rect(100.0, 100.0, 1000.0, 10.0);
		let others = [rect(1108.0, 115.0, 50.0, 50.0)];
		let snapped = snap.apply(bottom_right, strip, &others, &CANVAS).rect;
		assert_eq!((snapped.x, snapped.y, snapped.width), (100.0, 100.0, 1008.0));
		assert!((snapped.height - 10.08).abs() < 1e-3);
		// With only the bottom's snap near, it stays as dragged.
		let others = [rect(1300.0, 115.0, 50.0, 50.0)];
		assert_eq!(snap.apply(bottom_right, strip, &others, &CANVAS).rect, strip);
	}

	#[test]
	fn a_posed_item_snaps_where_it_shows_and_keeps_its_shape_at_rest() {
		// Resting at 200x100, with a keyframe adding 100 to the height.
		let snap = Snap {
			aspect: Some(2.0),
			posed: rect(0.0, 0.0, 0.0, 100.0),
			..SNAP
		};
		let right = Grab::Resize {
			x: Some(Side::End),
			y: None,
		};
		let dragged = right.drag(rect(100.0, 100.0, 200.0, 100.0), [20.0, 0.0], snap.aspect);
		assert_eq!(dragged, rect(100.0, 95.0, 220.0, 110.0));
		// Its right edge already lies on the other item's left edge.
		let others = [rect(320.0, 600.0, 100.0, 100.0)];
		let snapped = snap.apply(right, dragged, &others, &CANVAS);
		assert_eq!(snapped.rect, dragged);
		assert_eq!(snapped.guides.len(), 1);
		// Offset where it shows, it snaps there and keeps its shape at rest.
		let snap = Snap {
			posed: rect(4.0, 0.0, 0.0, 100.0),
			..snap
		};
		let snapped = snap.apply(right, dragged, &others, &CANVAS);
		assert_eq!(snapped.rect, rect(100.0, 96.0, 216.0, 108.0));
	}

	#[test]
	fn moves_snap_to_the_canvas_center_and_edges() {
		// Its center is 3 px right of the canvas's and its top 4 px below it.
		let snapped = SNAP.apply(Grab::Move, rect(863.0, 4.0, 200.0, 100.0), &[], &CANVAS);
		assert_eq!(snapped.rect, rect(860.0, 0.0, 200.0, 100.0));
		let guides = [
			Guide {
				axis: Axis::X,
				at: 960.0,
				from: 0.0,
				to: 1080.0,
			},
			Guide {
				axis: Axis::Y,
				at: 0.0,
				from: 0.0,
				to: 1920.0,
			},
		];
		assert_eq!(snapped.guides, guides);
	}

	#[test]
	fn out_of_reach_stays_put() {
		let item = rect(500.0, 700.0, 100.0, 100.0);
		let snapped = SNAP.apply(Grab::Move, item, &[], &CANVAS);
		assert_eq!(snapped.rect, item);
		assert!(snapped.guides.is_empty());
	}

	#[test]
	fn the_nearest_line_wins() {
		// Its left edge is 6 px right of one item's right edge, and its right
		// edge 2 px left of another's center.
		let others = [rect(0.0, 0.0, 494.0, 50.0), rect(560.0, 300.0, 84.0, 50.0)];
		let snapped = SNAP.apply(Grab::Move, rect(500.0, 600.0, 100.0, 100.0), &others, &CANVAS);
		assert_eq!(snapped.rect, rect(502.0, 600.0, 100.0, 100.0));
		// Spanning from the other item down through the dragged one.
		let guide = Guide {
			axis: Axis::X,
			at: 602.0,
			from: 300.0,
			to: 700.0,
		};
		assert_eq!(snapped.guides, [guide]);
	}

	#[test]
	fn a_resize_snaps_only_the_edge_it_holds() {
		let others = [rect(0.0, 300.0, 405.0, 50.0)];
		let right = Grab::Resize {
			x: Some(Side::End),
			y: None,
		};
		// The right edge at 398 snaps to the other's right edge at 405, though
		// the left edge sits 2 px from the canvas edge.
		let snapped = SNAP.apply(right, rect(2.0, 500.0, 396.0, 100.0), &others, &CANVAS);
		assert_eq!(snapped.rect, rect(2.0, 500.0, 403.0, 100.0));
		assert_eq!(snapped.guides.len(), 1);
	}

	#[test]
	fn the_grid() {
		let snap = Snap {
			grid: Some(64.0),
			..SNAP
		};
		// The left edge is 6 px from 256, and the top 2 px from 320.
		let snapped = snap.apply(Grab::Move, rect(250.0, 322.0, 100.0, 100.0), &[], &CANVAS);
		assert_eq!(snapped.rect, rect(256.0, 320.0, 100.0, 100.0));
		// The grid is drawn, so it adds no guides.
		assert!(snapped.guides.is_empty());
	}

	#[test]
	fn centers_between_two_neighbors() {
		let others = [rect(100.0, 500.0, 100.0, 100.0), rect(700.0, 500.0, 100.0, 100.0)];
		// The space is 500 wide; centered, a 100-wide item sits at 400.
		let snapped = SNAP.apply(Grab::Move, rect(395.0, 520.0, 100.0, 100.0), &others, &CANVAS);
		assert_eq!(snapped.rect.x, 400.0);
		assert_eq!(spaces(&snapped), [(200.0, 400.0), (500.0, 700.0)]);
		// Resizing the same item is not pulled by the spacing it would have moved to.
		let right = Grab::Resize {
			x: Some(Side::End),
			y: None,
		};
		let item = rect(395.0, 520.0, 100.0, 100.0);
		let snapped = SNAP.apply(right, item, &others, &CANVAS);
		assert_eq!(snapped.rect, item);
		assert!(snapped.gaps.is_empty());
	}

	#[test]
	fn repeats_the_spacing_of_a_pair() {
		let others = [rect(110.0, 500.0, 100.0, 100.0), rect(310.0, 500.0, 100.0, 100.0)];
		// The pair is 100 apart, so the next one belongs at 510.
		let snapped = SNAP.apply(Grab::Move, rect(517.0, 500.0, 50.0, 100.0), &others, &CANVAS);
		assert_eq!(snapped.rect.x, 510.0);
		assert_eq!(spaces(&snapped), [(410.0, 510.0), (210.0, 310.0)]);
		// And before the pair, at 110 - 100 - 50.
		let snapped = SNAP.apply(Grab::Move, rect(-44.0, 500.0, 50.0, 100.0), &others, &CANVAS);
		assert_eq!(snapped.rect.x, -40.0);
		assert_eq!(spaces(&snapped), [(10.0, 110.0), (210.0, 310.0)]);
	}

	#[test]
	fn spacing_ignores_items_in_another_row() {
		let others = [rect(100.0, 0.0, 100.0, 100.0), rect(300.0, 0.0, 100.0, 100.0)];
		let snapped = SNAP.apply(Grab::Move, rect(507.0, 500.0, 50.0, 100.0), &others, &CANVAS);
		assert_eq!(snapped.rect.x, 507.0);
		assert!(snapped.gaps.is_empty());
	}

	#[test]
	fn hits_the_selected_edges_then_the_front() {
		let backdrop = Item::new("backdrop", rect(0.0, 0.0, 1920.0, 1080.0));
		let mut hidden = Item::new("hidden", rect(500.0, 500.0, 200.0, 200.0));
		hidden.visible = false;
		let front = Item::new("front", rect(400.0, 400.0, 400.0, 400.0));
		let items = [backdrop, hidden, front];
		let right = Grab::Resize {
			x: Some(Side::End),
			y: None,
		};

		// A selected backdrop's inside yields to the item in front of it.
		assert_eq!(hit(&items, Some(0), [600.0, 600.0], 8.0), Some((2, Grab::Move)));
		assert_eq!(hit(&items, Some(0), [1915.0, 600.0], 8.0), Some((0, right)));
		// The selected item's edge reaches past the item in front of it.
		let mut selected = items.clone();
		selected[0].rect = rect(0.0, 0.0, 600.0, 1080.0);
		assert_eq!(hit(&selected, Some(0), [603.0, 600.0], 8.0), Some((0, right)));
		// A hidden selected item's edge, inside a visible item, grabs that item.
		assert_eq!(hit(&items, Some(1), [700.0, 600.0], 8.0), Some((2, Grab::Move)));
		// Outside everything shown, nothing.
		let items = &items[1..2];
		assert_eq!(hit(items, Some(0), [700.0, 600.0], 8.0), None);
	}

	#[test]
	fn spacing_is_not_offered_through_an_item() {
		// Repeating the pair's spacing would land on a third item.
		let others = [
			rect(100.0, 500.0, 100.0, 100.0),
			rect(300.0, 500.0, 100.0, 100.0),
			rect(440.0, 500.0, 100.0, 100.0),
		];
		let snapped = SNAP.apply(Grab::Move, rect(503.0, 500.0, 50.0, 100.0), &others, &CANVAS);
		assert_eq!(snapped.rect.x, 503.0);
		assert!(snapped.gaps.is_empty());
	}

	fn spaces(snapped: &Snapped) -> Vec<(f32, f32)> {
		snapped.gaps.iter().map(|gap| (gap.start, gap.end)).collect()
	}
}
