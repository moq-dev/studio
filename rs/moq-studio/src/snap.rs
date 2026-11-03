//! Where an item dragged in the preview lands: the part of it the pointer
//! holds, and how it snaps to the canvas, other items, and a grid.
//!
//! Plain math in canvas pixels with no egui types, so it is tested here; the
//! preview only converts the pointer and draws the result.

use moq_scene::{Canvas, Rect};

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
	pub fn drag(self, rect: Rect, delta: [f32; 2]) -> Rect {
		let mut rect = rect;
		for axis in Axis::ALL {
			shift(&mut rect, axis, self.moves(axis), axis.of(delta));
		}
		rect
	}
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

/// How a drag snaps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
	/// How far, in canvas pixels, an edge or center is pulled to a line it nears.
	pub reach: f32,
	/// The spacing of a grid to snap to, in canvas pixels, if any.
	pub grid: Option<f32>,
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
	/// Snap `rect`, which `grab` has dragged, to the canvas, the `others`, and
	/// the grid. Along each axis, the nearest line within reach of a moving
	/// edge or center wins, as does the spacing of a neighboring pair when the
	/// whole item moves.
	pub fn apply(&self, grab: Grab, rect: Rect, others: &[Rect], canvas: &Canvas) -> Snapped {
		let mut rect = rect;
		for axis in Axis::ALL {
			let moves = grab.moves(axis);
			let points = points(axis.span(&rect), moves);
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
				shifts.extend(spacings(axis, &rect, others));
			}
			let best = shifts
				.into_iter()
				.filter(|shift| shift.abs() <= self.reach)
				.min_by(|a, b| a.abs().total_cmp(&b.abs()));
			if let Some(best) = best {
				shift(&mut rect, axis, moves, best);
			}
		}

		let mut guides = Vec::new();
		let mut gaps = Vec::new();
		for axis in Axis::ALL {
			let moves = grab.moves(axis);
			guides.extend(lined_up(axis, &rect, &points(axis.span(&rect), moves), others, canvas));
			if moves == Moves::Both {
				gaps.extend(equal_gaps(axis, &rect, others));
			}
		}
		Snapped { rect, guides, gaps }
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

	fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
		Rect { x, y, width, height }
	}

	const SNAP: Snap = Snap {
		reach: 10.0,
		grid: None,
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
		assert_eq!(left.drag(item, [500.0, 40.0]), rect(300.0, 100.0, 0.0, 100.0));
		let bottom_right = Grab::Resize {
			x: Some(Side::End),
			y: Some(Side::End),
		};
		assert_eq!(bottom_right.drag(item, [10.0, -20.0]), rect(100.0, 100.0, 210.0, 80.0));
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
			reach: 10.0,
			grid: Some(64.0),
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
