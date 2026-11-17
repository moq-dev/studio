//! Where an item that keeps out of the way goes.
//!
//! A few times a second, the compositor draws the picture under the item,
//! scaled far down, and a compute shader reduces it, with the read before, to
//! a grid of tiles: how much of each moved, how much of it is edges, as text
//! and a HUD are, and how much of those held still. Only the grid comes back,
//! a frame or so later. A [`Placer`] weighs it into what matters where, and
//! keeps the item in the [`Slot`] that covers the least of it: it chooses
//! when its scene goes on and when the content under it changes, and moves
//! otherwise only once its place has been the worst for a while, gliding
//! there. A move by hand holds until the content changes.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use kio::clock::Instant;

use crate::collection::{Canvas, Item, KeepClear, Rect, mix};
use crate::compositor::Target;
use crate::effect::Effects;
use crate::glide::Glide;

/// How often the picture under an item is read, at most.
pub const INTERVAL: Duration = Duration::from_millis(250);

/// How long a gesture holds an item keeping clear in hand after it last
/// said so: a window that stops drawing mid-drag, as a hidden one does,
/// lets go this long after.
pub const GRAB: Duration = Duration::from_millis(500);

/// The longest side of the picture read, in pixels: text a 1080p canvas
/// shows at a readable size still has edges at this size.
const SIDE: u32 = 512;

/// A tile's side in pixels of the picture read, as `place.wgsl`'s workgroup.
const TILE: u32 = 16;

/// The bytes of the largest grid, four a tile.
const BYTES: u64 = ((SIDE / TILE) * (SIDE / TILE) * 4) as u64;

/// How long what matters lingers once it stops, so one busy moment is never
/// a reason to move.
const LINGER: Duration = Duration::from_secs(2);

/// How long the layout of the picture is averaged over to tell a new kind of
/// content from the one before, which is averaged over [`USUAL`].
const RECENT: Duration = Duration::from_millis(500);

/// How long the layout the content usually has is averaged over.
const USUAL: Duration = Duration::from_secs(6);

/// How far apart the recent and usual layouts get, per tile on average out of
/// 1 for its brightness and its edges, once the content changed kind.
const APART: f32 = 0.12;

/// How long the layouts stay [`APART`] before the content counts as changed:
/// a flash or a fast pan passes sooner.
const CHANGING: Duration = Duration::from_secs(1);

/// How long an item looks, once its scene goes on or the content under it
/// changes, before it chooses: long enough to see what moves.
pub const SETTLE: Duration = Duration::from_secs(1);

/// How long its place must be the worst before it moves without the content
/// changing.
pub const WORST: Duration = Duration::from_secs(10);

/// How much less a place must matter than where the item is for it to move,
/// on a tile's 0 to 1 scale, so it never wavers between two alike.
const BETTER: f32 = 0.1;

/// How close two places' costs are to count as alike, past the rounding of
/// summing them tile by tile.
const ALIKE: f32 = 1e-4;

/// What a slot along an edge's middle costs over a corner covering as much:
/// the middle of an edge is where captions and the game's own bars go.
const MIDDLE: f32 = 0.05;

/// How much each signal weighs at full strength: a HUD that holds still is
/// what viewers look for, and motion is everywhere in a game.
const WEIGHT: [f32; 3] = [0.5, 0.8, 1.0];

/// A place along the canvas's edges an item can keep to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
	TopLeft,
	Top,
	TopRight,
	Left,
	Right,
	BottomLeft,
	Bottom,
	BottomRight,
}

impl Slot {
	/// Every slot, corners first.
	pub const ALL: [Self; 8] = [
		Self::TopLeft,
		Self::TopRight,
		Self::BottomLeft,
		Self::BottomRight,
		Self::Top,
		Self::Left,
		Self::Right,
		Self::Bottom,
	];

	/// Whether it is a corner, which a middle slot yields to when both cover
	/// as much.
	pub fn corner(self) -> bool {
		matches!(
			self,
			Self::TopLeft | Self::TopRight | Self::BottomLeft | Self::BottomRight
		)
	}

	/// Its column and row in a 3x3 grid over the canvas, from the top left.
	pub fn cell(self) -> (u8, u8) {
		match self {
			Self::TopLeft => (0, 0),
			Self::Top => (1, 0),
			Self::TopRight => (2, 0),
			Self::Left => (0, 1),
			Self::Right => (2, 1),
			Self::BottomLeft => (0, 2),
			Self::Bottom => (1, 2),
			Self::BottomRight => (2, 2),
		}
	}

	/// Where an item `width` by `height` sits in it on `canvas`, inside the
	/// [`margin`], or `None` if it does not fit there.
	pub fn rect(self, width: f32, height: f32, canvas: Canvas) -> Option<Rect> {
		let margin = margin(canvas);
		let along = |cell: u8, length: f32, size: f32| match cell {
			0 => margin,
			1 => ((length - size) / 2.0).round(),
			_ => length - margin - size,
		};
		let (w, h) = (canvas.width as f32, canvas.height as f32);
		let fits = width > 0.0 && height > 0.0 && width + 2.0 * margin <= w && height + 2.0 * margin <= h;
		let (column, row) = self.cell();
		fits.then(|| Rect {
			x: along(column, w, width),
			y: along(row, h, height),
			width,
			height,
		})
	}
}

/// How far in from the canvas's edges a slot keeps, in pixels: a twentieth
/// of its shorter side.
pub fn margin(canvas: Canvas) -> f32 {
	(canvas.width.min(canvas.height) as f32 / 20.0).round()
}

/// What makes part of a picture matter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Signal {
	/// It moves.
	Motion,
	/// It is dense with edges that hold still, as text is.
	Text,
	/// It holds still while the rest moves, as a game's HUD or minimap does.
	Hud,
}

impl Signal {
	const ALL: [Self; 3] = [Self::Motion, Self::Text, Self::Hud];

	/// Whether `keep` stays clear of it.
	fn kept(self, keep: &KeepClear) -> bool {
		match self {
			Self::Motion => keep.motion,
			Self::Text => keep.text,
			Self::Hud => keep.hud,
		}
	}
}

/// What matters in the picture under an item, as a grid of tiles over its
/// canvas.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Heat {
	/// Tiles across.
	pub columns: usize,
	/// Tiles down.
	pub rows: usize,
	/// Each tile, row by row: how much it matters, from 0 to 1, and the
	/// signal it matters most for.
	pub tiles: Vec<(f32, Signal)>,
	/// Where each tile is on the canvas, as fractions of its width and height
	/// at the tile edges: `columns + 1` across, then `rows + 1` down.
	pub edges: (Vec<f32>, Vec<f32>),
}

impl Heat {
	/// The tile at `column`, `row` on the canvas, as fractions of its size.
	pub fn tile(&self, column: usize, row: usize) -> (f32, f32, f32, f32) {
		let (across, down) = &self.edges;
		(across[column], down[row], across[column + 1], down[row + 1])
	}
}

/// Where an item keeping clear is drawn, and why, as of the last render.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
	/// Where it is drawn, part way along a glide while one is under way.
	pub rect: Rect,
	/// The slot it is in or gliding to, or `None` while it stays where it
	/// was put.
	pub slot: Option<Slot>,
	/// Whether it stays where the streamer moved it, until the content under
	/// it changes.
	pub held: bool,
	/// Where it last moved from, and what it left there.
	pub left: Option<Left>,
	/// The places it may go: each slot it fits in.
	pub slots: Vec<(Slot, Rect)>,
	/// What matters under it, once it has looked.
	pub heat: Heat,
}

/// Where an item moved from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Left {
	/// Where it was.
	pub rect: Rect,
	/// What mattered most there, if anything did.
	pub over: Option<Signal>,
}

/// One read of the picture under an item: each tile's share of pixels that
/// moved, that are edges, and that are edges that held still, and its
/// brightness, each out of 255.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tiles {
	/// The picture's size in pixels, which the tiles cover [`TILE`] at a
	/// time, those at the right and bottom only as far as it goes.
	pub size: (u32, u32),
	/// Whether there was a read before to compare with, without which
	/// nothing moved.
	pub compared: bool,
	pub tiles: Vec<[u8; 4]>,
}

impl Tiles {
	const MOVED: usize = 0;
	const EDGES: usize = 1;
	const STILL: usize = 2;
	const LIGHT: usize = 3;
}

/// The fraction `amount` of the way to `full`, at most 1.
fn ramp(amount: f32, full: f32) -> f32 {
	(amount / full).clamp(0.0, 1.0)
}

/// How much of the way to a new value an average kept over `over` goes when
/// a value arrives `dt` after the last.
fn weight(dt: Duration, over: Duration) -> f32 {
	1.0 - (-dt.as_secs_f32() / over.as_secs_f32()).exp()
}

/// What the reads of the picture under an item show over time.
#[derive(Default)]
struct Sight {
	/// The picture size the reads are of; a read of another starts over.
	size: (u32, u32),
	/// Each tile's signals, lingering, in [`Signal::ALL`]'s order.
	signals: Vec<[f32; 3]>,
	/// Each tile's brightness and edges, recently and usually.
	recent: Vec<[f32; 2]>,
	usual: Vec<[f32; 2]>,
	/// Since when the recent and usual layouts have been [`APART`].
	apart: Option<Instant>,
	/// When the last read was taken.
	last: Option<Instant>,
	/// How many reads compared with the one before since it started over.
	compared: u32,
	/// Bumped with every read, so what is drawn from it is redrawn.
	generation: u64,
}

impl Sight {
	/// Take in a read taken `at`, returning whether the content changed kind.
	fn see(&mut self, read: &Tiles, at: Instant) -> bool {
		self.generation += 1;
		let count = read.tiles.len();
		if read.size != self.size || self.signals.len() != count {
			*self = Self {
				size: read.size,
				generation: self.generation,
				..Self::default()
			};
		}
		let unit = |value: u8| f32::from(value) / 255.0;
		let moving = match read.compared {
			true => read.tiles.iter().map(|tile| unit(tile[Tiles::MOVED])).sum::<f32>() / count.max(1) as f32,
			false => 0.0,
		};
		// Edges that hold still are text, and a HUD while the rest moves;
		// edges that move are motion. A first read, with nothing to compare,
		// takes every edge for still.
		let hud = ramp(moving - 0.03, 0.1);
		let raw = read.tiles.iter().map(|tile| {
			let (moved, still) = match read.compared {
				true => (unit(tile[Tiles::MOVED]), unit(tile[Tiles::STILL])),
				false => (0.0, unit(tile[Tiles::EDGES])),
			};
			[ramp(moved, 0.3), ramp(still, 0.2), ramp(still, 0.15) * hud]
		});
		let layout = read
			.tiles
			.iter()
			.map(|tile| [unit(tile[Tiles::LIGHT]), unit(tile[Tiles::EDGES])]);

		let Some(last) = self.last.replace(at) else {
			self.signals = raw.collect();
			self.recent = layout.collect();
			self.usual = self.recent.clone();
			self.compared = u32::from(read.compared);
			return false;
		};
		let dt = at.saturating_duration_since(last);
		self.compared += u32::from(read.compared);
		let linger = weight(dt, LINGER);
		for (signals, raw) in self.signals.iter_mut().zip(raw) {
			for (signal, raw) in signals.iter_mut().zip(raw) {
				// A signal rises at once and lingers as it falls, so what
				// matters counts from the first read that sees it.
				*signal = if raw > *signal { raw } else { mix(*signal, raw, linger) };
			}
		}
		let (recent, usual) = (weight(dt, RECENT), weight(dt, USUAL));
		let mut distance = 0.0;
		for ((now, usually), layout) in self.recent.iter_mut().zip(&mut self.usual).zip(layout) {
			for channel in 0..2 {
				now[channel] = mix(now[channel], layout[channel], recent);
				usually[channel] = mix(usually[channel], layout[channel], usual);
				distance += (now[channel] - usually[channel]).abs();
			}
		}
		distance /= count.max(1) as f32;
		if distance < APART {
			self.apart = None;
			return false;
		}
		let apart = *self.apart.get_or_insert(at);
		if at.saturating_duration_since(apart) < CHANGING {
			return false;
		}
		// What it usually looks like is what it looks like now, and what
		// mattered in the content before matters no more.
		self.usual = self.recent.clone();
		self.signals = self.recent.iter().map(|_| [0.0; 3]).collect();
		self.apart = None;
		self.compared = 0;
		true
	}

	/// Forget what was seen, as when the scene goes on again.
	fn forget(&mut self) {
		*self = Self {
			generation: self.generation + 1,
			..Self::default()
		};
	}

	/// How much each tile matters to an item keeping clear as `keep` says,
	/// and for what most, or `None` before any read.
	fn heat(&self, keep: &KeepClear) -> Option<Heat> {
		self.last?;
		let tiles = self
			.signals
			.iter()
			.map(|signals| {
				let weighed = Signal::ALL.map(|signal| {
					let index = signal as usize;
					let weight = if signal.kept(keep) { WEIGHT[index] } else { 0.0 };
					(signals[index] * weight, signal)
				});
				weighed
					.into_iter()
					.max_by(|a, b| a.0.total_cmp(&b.0))
					.expect("three signals")
			})
			.collect();
		let (width, height) = self.size;
		let fractions = |length: u32| -> Vec<f32> {
			let tiles = length.div_ceil(TILE);
			(0..=tiles)
				.map(|tile| (tile * TILE).min(length) as f32 / length as f32)
				.collect()
		};
		Some(Heat {
			columns: width.div_ceil(TILE) as usize,
			rows: height.div_ceil(TILE) as usize,
			tiles,
			edges: (fractions(width), fractions(height)),
		})
	}
}

/// How much `rect` on `canvas` covers of what matters in `heat`: the mean of
/// its tiles, each by how much of it the rect covers, and what mattered most
/// under it.
fn covered(heat: &Heat, rect: &Rect, canvas: Canvas) -> (f32, Option<Signal>) {
	let (w, h) = (canvas.width as f32, canvas.height as f32);
	let (left, top) = (rect.x / w, rect.y / h);
	let (right, bottom) = ((rect.x + rect.width) / w, (rect.y + rect.height) / h);
	let area = (right - left) * (bottom - top);
	if area <= 0.0 {
		return (0.0, None);
	}
	let mut sum = 0.0;
	let mut most = (0.0, None);
	for row in 0..heat.rows {
		for column in 0..heat.columns {
			let (x0, y0, x1, y1) = heat.tile(column, row);
			let overlap = (x1.min(right) - x0.max(left)).max(0.0) * (y1.min(bottom) - y0.max(top)).max(0.0);
			if overlap <= 0.0 {
				continue;
			}
			let (matters, signal) = heat.tiles[row * heat.columns + column];
			sum += matters * overlap;
			if matters > most.0 {
				most = (matters, Some(signal));
			}
		}
	}
	(sum / area, most.1)
}

/// Where an item is meant to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum At {
	/// Where its saved rect puts it, before it first chooses.
	Rest,
	/// Where the streamer moved it, until the content changes.
	Held,
	/// In a slot.
	Slot(Slot),
}

/// What a move by hand held an item from, so an undo back there can put it
/// back.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Before {
	/// Where its scene file put it.
	saved: Rect,
	/// Where it was meant to be, such as the slot it had glided to.
	at: At,
	/// Whether it was looking before choosing.
	looking: bool,
}

/// Chooses where one item keeping clear goes, from what reads of the picture
/// under it show.
pub(crate) struct Placer {
	sight: Sight,
	at: At,
	/// When it last started looking before choosing: when it began, its
	/// scene went on, or the content under it changed.
	looking: Option<Instant>,
	/// Since when its place has been the worst.
	worst: Option<Instant>,
	/// How long its scene had been on as of the last read, which tells it
	/// going on again.
	since: Option<Duration>,
	/// The item gliding from one place to the next as a move transition
	/// eases, keyed by how many times it moved.
	glide: Glide<u64>,
	/// How long [`glide`](Self::glide) takes.
	glides: Duration,
	moves: u64,
	/// Where it was meant to be as of the last [`place`](Self::place), and
	/// after how many moves: another place with no move, as a resize gives,
	/// is taken at once, even part way along a glide.
	aimed: Option<(Rect, u64)>,
	/// Whether its picture is read: its scene drawn and the item shown. It
	/// chooses only while it is.
	watched: bool,
	/// Whether a gesture has it in hand, such as a drag: it chooses only once
	/// let go.
	grabbed: bool,
	/// Until when the glide of its last choice runs.
	gliding: Option<Instant>,
	/// Where a glide under way stopped when a gesture took it in hand: it
	/// stays there, at the size it is meant to have, until let go, then
	/// glides on from there.
	paused: Option<Rect>,
	placement: Placement,
	/// Where its scene file put it as of the last [`place`](Self::place).
	saved: Option<Rect>,
	/// What the move by hand that holds it, or last held it, held it from.
	before: Option<Before>,
	/// What [`Sight::generation`] and settings the heat was drawn from.
	heated: Option<(u64, KeepClear)>,
}

impl Placer {
	/// A placer that starts looking at `now`.
	pub fn new(now: Instant) -> Self {
		let nowhere = Rect {
			x: 0.0,
			y: 0.0,
			width: 0.0,
			height: 0.0,
		};
		Self {
			sight: Sight::default(),
			at: At::Rest,
			looking: Some(now),
			worst: None,
			since: None,
			glide: Glide::new(KeepClear::default().glide, |_| true),
			glides: KeepClear::default().glide,
			moves: 0,
			aimed: None,
			watched: true,
			grabbed: false,
			gliding: None,
			paused: None,
			placement: Placement {
				rect: nowhere,
				slot: None,
				held: false,
				left: None,
				slots: Vec::new(),
				heat: Heat::default(),
			},
			saved: None,
			before: None,
			heated: None,
		}
	}

	/// Take in a read of the picture under the item, taken `at`.
	pub fn see(&mut self, read: &Tiles, at: Instant) {
		if self.sight.see(read, at) {
			self.looking = Some(at);
		}
	}

	/// Note that the item's scene is drawn, on for `since`, as of `now`: one
	/// that went on again since the last is new content to choose for.
	pub fn shown(&mut self, since: Duration, now: Instant) {
		if self.since.is_some_and(|last| since < last) {
			self.sight.forget();
			self.looking = Some(now);
		}
		self.since = Some(since);
	}

	/// Moved by hand, such as dragged or nudged: it stays where the scene
	/// file has it until the content under it changes, and a choice it was
	/// about to make is dropped. A resize alone is no move: it keeps its
	/// place. The first move of a hold remembers where it was, for
	/// [`undone`](Self::undone).
	pub fn hold(&mut self) {
		if self.at != At::Held
			&& let Some(saved) = self.saved
		{
			self.before = Some(Before {
				saved,
				at: self.at,
				looking: self.looking.is_some(),
			});
		}
		self.at = At::Held;
		self.looking = None;
		self.worst = None;
		self.glide = Glide::new(self.glides, |_| true);
		self.gliding = None;
		self.paused = None;
	}

	/// Moved by an undo, saved at `saved` as of `now`: back where its scene
	/// file put it before the move by hand that held it, it goes back to
	/// where it was meant to be then, such as the slot it had glided to, so
	/// the undo puts back what the streamer saw. Anywhere else, it holds, as
	/// after a move by hand.
	pub fn undone(&mut self, saved: Rect, now: Instant) {
		match self.before.take() {
			Some(before) if before.saved == saved => {
				self.at = before.at;
				self.looking = before.looking.then_some(now);
				self.worst = None;
				self.glide = Glide::new(self.glides, |_| true);
				self.gliding = None;
				self.paused = None;
			}
			before => {
				self.hold();
				// What it was held from is still what an undo further back
				// returns to.
				self.before = before;
			}
		}
	}

	/// Note whether its picture is read as of `now`: while its scene is not
	/// drawn or the item is hidden it sees nothing new, so it neither
	/// chooses nor counts its place the worst, and it looks again once it is
	/// read again, unless a move by hand holds it, which only the content
	/// changing ends.
	pub fn watch(&mut self, watched: bool, now: Instant) {
		if watched && !self.watched && self.at != At::Held {
			self.looking = Some(now);
		}
		if !watched {
			self.worst = None;
		}
		self.watched = watched;
	}

	/// Note whether a gesture has it in hand, such as a drag that moves,
	/// resizes, or crops it: while it does, it neither chooses nor counts its
	/// place the worst, and once let go it goes on as the gesture left it, a
	/// move holding it.
	pub fn grab(&mut self, grabbed: bool) {
		if grabbed {
			self.worst = None;
		}
		self.grabbed = grabbed;
	}

	/// Where it is as of the last [`place`](Self::place).
	pub fn placement(&self) -> &Placement {
		&self.placement
	}

	/// Where to draw the item at `now`, saved at `saved` and keeping clear as
	/// `keep` says on `canvas`.
	pub fn place(&mut self, saved: Rect, keep: &KeepClear, canvas: Canvas, now: Instant) -> Rect {
		self.saved = Some(saved);
		let slots: Vec<(Slot, Rect)> = Slot::ALL
			.into_iter()
			.filter_map(|slot| Some((slot, slot.rect(saved.width, saved.height, canvas)?)))
			.collect();
		if let At::Slot(slot) = self.at
			&& !slots.iter().any(|(fits, _)| *fits == slot)
		{
			self.at = At::Rest;
		}

		let generation = (self.sight.generation, *keep);
		if self.heated != Some(generation) {
			self.heated = Some(generation);
			self.placement.heat = self.sight.heat(keep).unwrap_or_default();
		}
		if self.watched && !self.grabbed && self.sight.last.is_some() && !slots.is_empty() {
			self.choose(saved, &slots, keep, canvas, now);
		}

		let target = self.target(saved, &slots);
		// Taken in hand part way along a glide, as a crop started then is, it
		// stops where it is drawn, so it stays under the pointer; let go, it
		// glides on from there.
		let gliding = self.gliding.is_some_and(|until| now < until);
		if self.grabbed && gliding && self.paused.is_none() {
			self.paused = Some(self.placement.rect);
		}
		if !self.grabbed
			&& let Some(paused) = self.paused.take()
		{
			let at = Rect {
				width: target.width,
				height: target.height,
				..paused
			};
			self.glide = Glide::new(self.glides, |_| true);
			self.glide.show(self.moves, &[shown(at)], now);
			self.moves += 1;
			self.gliding = Some(now.saturating_add(self.glides));
		}
		let rect = match self.paused {
			Some(paused) => {
				self.aimed = Some((target, self.moves));
				Rect {
					width: target.width,
					height: target.height,
					..paused
				}
			}
			None => {
				// Another target with no move, as a resize gives, shows at
				// once, even part way along a glide.
				if self
					.aimed
					.is_some_and(|(aimed, moves)| aimed != target && moves == self.moves)
				{
					self.glide = Glide::new(self.glides, |_| true);
				}
				self.aimed = Some((target, self.moves));
				self.glide.show(self.moves, &[shown(target)], now);
				self.glide.placed()[0].rect
			}
		};
		self.placement.rect = rect;
		self.placement.slot = match self.at {
			At::Slot(slot) => Some(slot),
			_ => None,
		};
		self.placement.held = self.at == At::Held;
		self.placement.slots = slots;
		rect
	}

	/// Where the item is meant to be, gliding or not.
	fn target(&self, saved: Rect, slots: &[(Slot, Rect)]) -> Rect {
		match self.at {
			At::Slot(slot) => slots
				.iter()
				.find(|(fits, _)| *fits == slot)
				.map_or(saved, |(_, rect)| *rect),
			At::Rest | At::Held => saved,
		}
	}

	/// Move to the slot that matters least, if the time has come.
	fn choose(&mut self, saved: Rect, slots: &[(Slot, Rect)], keep: &KeepClear, canvas: Canvas, now: Instant) {
		let heat = &self.placement.heat;
		let cost = |slot: Option<Slot>, rect: &Rect| {
			let (covered, over) = covered(heat, rect, canvas);
			let middle = if slot.is_some_and(|slot| !slot.corner()) {
				MIDDLE
			} else {
				0.0
			};
			(covered + middle, over)
		};
		let costs: Vec<(Slot, f32)> = slots
			.iter()
			.map(|(slot, rect)| (*slot, cost(Some(*slot), rect).0))
			.collect();
		let lowest = costs.iter().map(|(_, cost)| *cost).fold(f32::INFINITY, f32::min);
		// Of slots that cover alike, the first: a corner, from the top left.
		let (best, _) = *costs
			.iter()
			.find(|(_, cost)| *cost <= lowest + ALIKE)
			.expect("a slot fits");
		let current = match self.at {
			At::Slot(slot) => Some(slot),
			_ => None,
		};
		let here = self.target(saved, slots);
		let (cost_here, over) = cost(current, &here);
		let better = lowest + BETTER <= cost_here && current != Some(best);

		let looked = self
			.looking
			.is_some_and(|since| now.saturating_duration_since(since) >= SETTLE && self.sight.compared >= 2);
		if looked {
			self.looking = None;
			self.worst = None;
			if self.at == At::Held {
				self.at = At::Rest;
			}
			if better {
				self.go(best, here, over, keep, now);
			}
			return;
		}
		if self.looking.is_some() || self.at == At::Held {
			return;
		}
		let worst = costs.iter().all(|(_, cost)| *cost <= cost_here + ALIKE);
		if !(worst && better) {
			self.worst = None;
			return;
		}
		let since = *self.worst.get_or_insert(now);
		if now.saturating_duration_since(since) >= WORST {
			self.worst = None;
			self.go(best, here, over, keep, now);
		}
	}

	/// Glide to `slot` from `here`, over what mattered most there.
	fn go(&mut self, slot: Slot, here: Rect, over: Option<Signal>, keep: &KeepClear, now: Instant) {
		self.placement.left = Some(Left { rect: here, over });
		self.at = At::Slot(slot);
		if keep.glide != self.glides {
			// Another length: a glide of that length, from where it is.
			self.glides = keep.glide;
			self.glide = Glide::new(keep.glide, |_| true);
			self.glide.show(self.moves, &[shown(self.placement.rect)], now);
		}
		self.moves += 1;
		self.gliding = Some(now.saturating_add(keep.glide));
	}
}

/// An item at `rect`, standing for the one keeping clear in its glide.
fn shown(rect: Rect) -> Item {
	Item::new("keeping clear", rect)
}

/// The pipeline every read of the picture under an item goes through.
pub(crate) struct Reducer {
	pipeline: wgpu::ComputePipeline,
	layout: wgpu::BindGroupLayout,
}

impl Reducer {
	pub fn new(device: &wgpu::Device) -> Self {
		let shader = device.create_shader_module(wgpu::include_wgsl!("place.wgsl"));
		let picture = |binding| wgpu::BindGroupLayoutEntry {
			binding,
			visibility: wgpu::ShaderStages::COMPUTE,
			ty: wgpu::BindingType::Texture {
				sample_type: wgpu::TextureSampleType::Float { filterable: false },
				view_dimension: wgpu::TextureViewDimension::D2,
				multisampled: false,
			},
			count: None,
		};
		let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("moq-scene place"),
			entries: &[
				picture(0),
				picture(1),
				wgpu::BindGroupLayoutEntry {
					binding: 2,
					visibility: wgpu::ShaderStages::COMPUTE,
					ty: wgpu::BindingType::Buffer {
						ty: wgpu::BufferBindingType::Storage { read_only: false },
						has_dynamic_offset: false,
						min_binding_size: wgpu::BufferSize::new(BYTES),
					},
					count: None,
				},
			],
		});
		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("moq-scene place"),
			bind_group_layouts: &[Some(&layout)],
			immediate_size: 0,
		});
		let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
			label: Some("moq-scene place"),
			layout: Some(&pipeline_layout),
			module: &shader,
			entry_point: Some("reduce"),
			compilation_options: Default::default(),
			cache: None,
		});
		Self { pipeline, layout }
	}
}

/// A read under way: when it was taken, its picture's size, whether it had
/// one before to compare with, and the buffer's mapping once the GPU is done.
struct Reading {
	at: Instant,
	size: (u32, u32),
	compared: bool,
	mapped: Arc<Mutex<Option<Result<(), wgpu::BufferAsyncError>>>>,
}

/// One item's reads of the picture under it.
pub(crate) struct Look {
	device: wgpu::Device,
	queue: wgpu::Queue,
	/// The pictures read, by turns: the next read draws into the one the read
	/// before last did, and compares with the other.
	pictures: [Option<Target>; 2],
	/// Which of `pictures` the next read draws into.
	next: usize,
	tiles: wgpu::Buffer,
	readback: wgpu::Buffer,
	reading: Option<Reading>,
	/// When the last read was taken.
	last: Option<Instant>,
}

impl Look {
	pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
		let tiles = device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("moq-scene place tiles"),
			size: BYTES,
			usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
			mapped_at_creation: false,
		});
		let readback = device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("moq-scene place tiles"),
			size: BYTES,
			usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
			mapped_at_creation: false,
		});
		Self {
			device: device.clone(),
			queue: queue.clone(),
			pictures: [None, None],
			next: 0,
			tiles,
			readback,
			reading: None,
			last: None,
		}
	}

	/// Whether a read may start at `now`: none is under way, and the last
	/// was at least [`INTERVAL`] ago.
	pub fn due(&self, now: Instant) -> bool {
		self.reading.is_none()
			&& self
				.last
				.is_none_or(|last| now.saturating_duration_since(last) >= INTERVAL)
	}

	/// What the next read draws the picture under the item into, `canvas`
	/// scaled down to at most [`SIDE`] a side.
	pub fn target(&mut self, effects: &Effects, canvas: Canvas) -> Target {
		let scale = (SIDE as f32 / canvas.width.max(canvas.height) as f32).min(1.0);
		let side = |length: u32| ((length as f32 * scale).round() as u32).max(1);
		let size = (side(canvas.width), side(canvas.height));
		let picture = &mut self.pictures[self.next];
		match picture {
			Some(target) if (target.texture.width(), target.texture.height()) == size => {
				target.canvas = canvas;
				target.clone()
			}
			_ => {
				// A picture of another size has nothing to compare with.
				let other = &mut self.pictures[1 - self.next];
				if other
					.as_ref()
					.is_some_and(|other| (other.texture.width(), other.texture.height()) != size)
				{
					*other = None;
				}
				let target = Target::sized(&self.device, effects, canvas, size, "moq-scene place");
				self.pictures[self.next].insert(target).clone()
			}
		}
	}

	/// Reduce the picture the frame drew into [`target`](Self::target), once
	/// submitted, with the one before, and read the tiles back.
	pub fn start(&mut self, reducer: &Reducer, now: Instant) {
		let read = self.pictures[self.next].as_ref().expect("drawn into");
		let before = self.pictures[1 - self.next].as_ref();
		let compared = before.is_some();
		let view = |target: &Target| target.texture.create_view(&Default::default());
		let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("moq-scene place"),
			layout: &reducer.layout,
			entries: &[
				wgpu::BindGroupEntry {
					binding: 0,
					resource: wgpu::BindingResource::TextureView(&view(read)),
				},
				wgpu::BindGroupEntry {
					binding: 1,
					resource: wgpu::BindingResource::TextureView(&view(before.unwrap_or(read))),
				},
				wgpu::BindGroupEntry {
					binding: 2,
					resource: self.tiles.as_entire_binding(),
				},
			],
		});
		let size = (read.texture.width(), read.texture.height());
		let mut encoder = self.device.create_command_encoder(&Default::default());
		{
			let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
				label: Some("moq-scene place"),
				timestamp_writes: None,
			});
			pass.set_pipeline(&reducer.pipeline);
			pass.set_bind_group(0, &bind, &[]);
			pass.dispatch_workgroups(size.0.div_ceil(TILE), size.1.div_ceil(TILE), 1);
		}
		let bytes = u64::from(size.0.div_ceil(TILE) * size.1.div_ceil(TILE) * 4);
		encoder.copy_buffer_to_buffer(&self.tiles, 0, &self.readback, 0, bytes);
		self.queue.submit([encoder.finish()]);

		let mapped = Arc::new(Mutex::new(None));
		let done = mapped.clone();
		// Called from a later submit or poll, once the GPU is done.
		self.readback.map_async(wgpu::MapMode::Read, .., move |result| {
			*done.lock().unwrap() = Some(result);
		});
		self.reading = Some(Reading {
			at: now,
			size,
			compared,
			mapped,
		});
		self.last = Some(now);
		self.next = 1 - self.next;
	}

	/// The read under way, once it came back, and when it was taken.
	pub fn finish(&mut self) -> Option<Result<(Tiles, Instant), String>> {
		let reading = self.reading.as_ref()?;
		let mapped = reading.mapped.lock().unwrap().take()?;
		let Reading { at, size, compared, .. } = self.reading.take().expect("checked above");
		let count = (size.0.div_ceil(TILE) * size.1.div_ceil(TILE)) as usize;
		let read = mapped.map_err(|err| err.to_string()).and_then(|()| {
			let tiles = match self.readback.slice(..).get_mapped_range() {
				Ok(view) => Ok(view.as_chunks::<4>().0[..count].to_vec()),
				Err(err) => Err(format!("{err:?}")),
			};
			self.readback.unmap();
			tiles
		});
		Some(read.map(|tiles| (Tiles { size, compared, tiles }, at)))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const CANVAS: Canvas = Canvas {
		width: 1920,
		height: 1080,
		framerate: 30,
	};

	/// The picture size a 1080p canvas is read at: 32 by 18 tiles.
	const SIZE: (u32, u32) = (512, 288);

	/// A read where every tile is `tile`, except those `marked` returns
	/// another for, by column and row.
	fn read(tile: [u8; 4], marked: impl Fn(usize, usize) -> Option<[u8; 4]>) -> Tiles {
		let mut tiles = Vec::new();
		for row in 0..18 {
			for column in 0..32 {
				tiles.push(marked(column, row).unwrap_or(tile));
			}
		}
		Tiles {
			size: SIZE,
			compared: true,
			tiles,
		}
	}

	/// A game: moving everywhere, with a minimap that holds still in the top
	/// right corner.
	fn game(column: usize, row: usize) -> Option<[u8; 4]> {
		(column >= 26 && row < 7).then_some([0, 60, 50, 90])
	}

	const MOVING: [u8; 4] = [120, 10, 0, 80];

	/// A slide: still, dark, with text along its bottom left.
	fn slide(column: usize, row: usize) -> Option<[u8; 4]> {
		(column < 14 && row >= 13).then_some([0, 70, 70, 200])
	}

	const STILL: [u8; 4] = [0, 0, 0, 20];

	fn camera() -> Rect {
		Rect {
			x: 1574.0,
			y: 54.0,
			width: 288.0,
			height: 288.0,
		}
	}

	/// A placer fed `frames` reads a quarter second apart from `start`,
	/// placing an item saved at `saved` after each; the time after the last.
	fn run(placer: &mut Placer, saved: Rect, start: Instant, reads: impl IntoIterator<Item = Tiles>) -> Instant {
		let mut now = start;
		for read in reads {
			placer.see(&read, now);
			placer.place(saved, &KeepClear::default(), CANVAS, now);
			now = now.saturating_add(INTERVAL);
		}
		now
	}

	#[test]
	fn slots_keep_inside_the_margin() {
		let margin = margin(CANVAS);
		assert_eq!(margin, 54.0);
		let top_left = Slot::TopLeft.rect(288.0, 288.0, CANVAS).unwrap();
		assert_eq!((top_left.x, top_left.y), (54.0, 54.0));
		let bottom = Slot::Bottom.rect(288.0, 288.0, CANVAS).unwrap();
		assert_eq!((bottom.x, bottom.y), (816.0, 738.0));
		let right = Slot::Right.rect(288.0, 288.0, CANVAS).unwrap();
		assert_eq!((right.x, right.y), (1578.0, 396.0));
		// Too tall to fit between the margins.
		assert_eq!(Slot::TopLeft.rect(288.0, 1000.0, CANVAS), None);
		assert_eq!(Slot::TopLeft.rect(0.0, 100.0, CANVAS), None);
	}

	#[test]
	fn a_rect_covers_the_tiles_under_it_by_area() {
		let mut sight = Sight::default();
		sight.see(&read(STILL, slide), Instant::now());
		let heat = sight.heat(&KeepClear::default()).unwrap();
		assert_eq!((heat.columns, heat.rows), (32, 18));
		let text = Slot::BottomLeft.rect(288.0, 288.0, CANVAS).unwrap();
		let (covered_text, over) = covered(&heat, &text, CANVAS);
		// Text weighs 0.8, over the part of the slot below the 13th row.
		assert!((covered_text - 0.8 * 246.0 / 288.0).abs() < 1e-3, "{covered_text}");
		assert_eq!(over, Some(Signal::Text));
		let clear = Slot::TopRight.rect(288.0, 288.0, CANVAS).unwrap();
		assert_eq!(covered(&heat, &clear, CANVAS), (0.0, None));
	}

	/// Edges that hold still while the rest moves are a HUD; the same edges
	/// in a still picture are text.
	#[test]
	fn still_edges_are_a_hud_only_while_the_rest_moves() {
		let start = Instant::now();
		let mut sight = Sight::default();
		sight.see(&read(MOVING, game), start);
		sight.see(&read(MOVING, game), start.saturating_add(INTERVAL));
		let heat = sight.heat(&KeepClear::default()).unwrap();
		assert_eq!(heat.tiles[30].1, Signal::Hud);
		assert_eq!(heat.tiles[0].1, Signal::Motion);

		let mut sight = Sight::default();
		let still = |column, row| game(column, row).map(|tile| [0, tile[1], tile[2], tile[3]]);
		sight.see(&read(STILL, still), start);
		sight.see(&read(STILL, still), start.saturating_add(INTERVAL));
		let heat = sight.heat(&KeepClear::default()).unwrap();
		assert_eq!(heat.tiles[30].1, Signal::Text);
	}

	/// Over a game's minimap, it waits a moment to see what moves, then
	/// glides to the corner that covers the least, easing as a move
	/// transition does.
	#[test]
	fn it_glides_off_a_hud_once_it_has_looked() {
		let start = Instant::now();
		let mut placer = Placer::new(start);
		let saved = camera();
		assert_eq!(placer.place(saved, &KeepClear::default(), CANVAS, start), saved);
		// Not yet: it has not looked for long enough.
		let now = run(&mut placer, saved, start, (0..3).map(|_| read(MOVING, game)));
		assert_eq!(placer.placement().slot, None);
		let now = run(&mut placer, saved, now, (0..2).map(|_| read(MOVING, game)));
		let placement = placer.placement().clone();
		assert_eq!(placement.slot, Some(Slot::TopLeft));
		let left = placement.left.unwrap();
		assert_eq!(left.rect, saved);
		assert_eq!(left.over, Some(Signal::Hud));

		let moved = now.saturating_sub(INTERVAL);
		let to = Slot::TopLeft.rect(288.0, 288.0, CANVAS).unwrap();
		let glide = KeepClear::default().glide;
		let half = placer.place(saved, &KeepClear::default(), CANVAS, moved.saturating_add(glide / 2));
		assert_eq!(half.x, (saved.x + to.x) / 2.0, "halfway, eased in and out");
		let landed = placer.place(saved, &KeepClear::default(), CANVAS, moved.saturating_add(glide));
		assert_eq!(landed, to);
	}

	/// It glides for as long as its settings say, or jumps.
	#[test]
	fn it_glides_as_long_as_it_is_set_to() {
		let to = Slot::TopLeft.rect(288.0, 288.0, CANVAS).unwrap();
		for glide in [Duration::ZERO, Duration::from_millis(1500)] {
			let keep = KeepClear {
				glide,
				..KeepClear::default()
			};
			let start = Instant::now();
			let mut placer = Placer::new(start);
			let saved = camera();
			let mut now = start;
			let mut moved = None;
			for _ in 0..8 {
				placer.see(&read(MOVING, game), now);
				placer.place(saved, &keep, CANVAS, now);
				if placer.placement().slot.is_some() {
					moved = Some(now);
					break;
				}
				now = now.saturating_add(INTERVAL);
			}
			let moved = moved.expect("it chose a slot");
			let first = placer.placement().rect;
			match glide.is_zero() {
				true => assert_eq!(first, to, "a jump lands at once"),
				false => {
					assert_eq!(first, saved, "a glide starts where it was");
					let half = placer.place(saved, &keep, CANVAS, moved.saturating_add(glide / 2));
					assert_eq!(half.x, (saved.x + to.x) / 2.0, "halfway at half its length");
					let landed = placer.place(saved, &keep, CANVAS, moved.saturating_add(glide));
					assert_eq!(landed, to);
				}
			}
		}
	}

	/// In hand, as a drag has it, it does not choose, though the time to has
	/// come; let go, it does.
	#[test]
	fn it_chooses_only_once_let_go() {
		let start = Instant::now();
		let mut placer = Placer::new(start);
		let saved = camera();
		placer.grab(true);
		let now = run(&mut placer, saved, start, (0..12).map(|_| read(MOVING, game)));
		assert_eq!(placer.placement().slot, None, "in hand, it stays");
		placer.grab(false);
		let now = run(&mut placer, saved, now, [read(MOVING, game)]);
		assert_eq!(placer.placement().slot, Some(Slot::TopLeft));
		let _ = now;
	}

	/// Taken in hand part way along a glide, as a crop started then is, it
	/// stops where it is drawn; let go, it glides on from there to its slot.
	#[test]
	fn a_glide_pauses_while_in_hand() {
		let start = Instant::now();
		let mut placer = Placer::new(start);
		let saved = camera();
		let now = run(&mut placer, saved, start, (0..5).map(|_| read(MOVING, game)));
		assert_eq!(placer.placement().slot, Some(Slot::TopLeft));
		let moved = now.saturating_sub(INTERVAL);
		let glide = KeepClear::default().glide;
		let to = Slot::TopLeft.rect(288.0, 288.0, CANVAS).unwrap();

		let half = moved.saturating_add(glide / 2);
		let at = placer.place(saved, &KeepClear::default(), CANVAS, half);
		assert!(at.x < saved.x && at.x > to.x, "on its way: {at:?}");
		placer.grab(true);
		for later in [half, half.saturating_add(glide), half.saturating_add(glide * 4)] {
			let held = placer.place(saved, &KeepClear::default(), CANVAS, later);
			assert_eq!(held, at, "in hand, it stays where it is drawn");
		}

		placer.grab(false);
		let released = half.saturating_add(glide * 4);
		assert_eq!(placer.place(saved, &KeepClear::default(), CANVAS, released), at);
		let on = placer.place(saved, &KeepClear::default(), CANVAS, released.saturating_add(glide / 2));
		assert!(on.x < at.x && on.x > to.x, "it glides on: {on:?}");
		let landed = placer.place(saved, &KeepClear::default(), CANVAS, released.saturating_add(glide));
		assert_eq!(landed, to);
	}

	/// A resize part way along a glide takes its new size at once, in the
	/// slot it was gliding to.
	#[test]
	fn a_resize_while_it_glides_lands_at_once() {
		let start = Instant::now();
		let mut placer = Placer::new(start);
		let saved = camera();
		let now = run(&mut placer, saved, start, (0..5).map(|_| read(MOVING, game)));
		assert_eq!(placer.placement().slot, Some(Slot::TopLeft));
		let resized = Rect {
			width: 200.0,
			height: 200.0,
			..saved
		};
		let rect = placer.place(resized, &KeepClear::default(), CANVAS, now);
		assert_eq!(Some(rect), Slot::TopLeft.rect(200.0, 200.0, CANVAS));
	}

	/// Unwatched, as when its scene is not drawn or it is hidden, it neither
	/// chooses nor counts toward [`WORST`], and it looks again once watched.
	#[test]
	fn it_chooses_only_while_watched() {
		let start = Instant::now();
		let mut placer = Placer::new(start);
		let saved = camera();
		placer.watch(false, start);
		let now = run(&mut placer, saved, start, (0..8).map(|_| read(MOVING, game)));
		assert_eq!(placer.placement().slot, None, "unwatched, it stays");
		let later = now.saturating_add(WORST * 2);
		placer.place(saved, &KeepClear::default(), CANVAS, later);
		assert_eq!(placer.placement().slot, None);

		placer.watch(true, later);
		placer.place(saved, &KeepClear::default(), CANVAS, later);
		assert_eq!(placer.placement().slot, None, "it looks again first");
		let now = run(&mut placer, saved, later, (0..8).map(|_| read(MOVING, game)));
		assert_eq!(placer.placement().slot, Some(Slot::TopLeft));

		// Moved by hand, then hidden and shown over the same game: it stays
		// where it was put.
		let dragged = Rect { x: 1600.0, ..saved };
		placer.hold();
		let now = run(&mut placer, dragged, now, (0..4).map(|_| read(MOVING, game)));
		placer.watch(false, now);
		let now = run(&mut placer, dragged, now, (0..4).map(|_| read(MOVING, game)));
		placer.watch(true, now);
		let now = run(&mut placer, dragged, now, (0..40).map(|_| read(MOVING, game)));
		let placement = placer.placement();
		assert!(placement.held, "still held");
		assert_eq!(placement.rect, dragged);
		let _ = now;
	}

	/// One busy read is never a reason to move: what it covers lingers, but
	/// only a place that stays the worst for [`WORST`] moves it.
	#[test]
	fn a_busy_moment_does_not_move_it() {
		let start = Instant::now();
		let mut placer = Placer::new(start);
		let saved = Slot::TopLeft.rect(288.0, 288.0, CANVAS).unwrap();
		let now = run(&mut placer, saved, start, (0..8).map(|_| read(MOVING, game)));
		assert_eq!(placer.placement().slot, None, "already clear");

		// Something flares under it for one read.
		let flare = |column, row| {
			(column < 10 && row < 8)
				.then_some([255, 200, 200, 200])
				.or(game(column, row))
		};
		let now = run(&mut placer, saved, now, [read(MOVING, flare)]);
		let now = run(&mut placer, saved, now, (0..80).map(|_| read(MOVING, game)));
		assert_eq!(placer.placement().slot, None);
		assert_eq!(placer.placement().rect, saved);

		// Something that stays there moves it, once it has been the worst
		// place for long enough.
		let reads = (WORST.as_millis() / INTERVAL.as_millis()) as usize;
		let now = run(&mut placer, saved, now, (0..reads - 4).map(|_| read(MOVING, flare)));
		assert_eq!(placer.placement().slot, None, "not yet");
		run(&mut placer, saved, now, (0..8).map(|_| read(MOVING, flare)));
		assert_ne!(placer.placement().slot, None);
		assert_eq!(placer.placement().left.unwrap().over, Some(Signal::Hud));
	}

	/// A move by hand holds, however much it covers, until the content under
	/// it changes; then it chooses again.
	#[test]
	fn a_move_by_hand_holds_until_the_content_changes() {
		let start = Instant::now();
		let mut placer = Placer::new(start);
		let saved = camera();
		let now = run(&mut placer, saved, start, (0..6).map(|_| read(MOVING, game)));
		assert_eq!(placer.placement().slot, Some(Slot::TopLeft));

		// Dragged back over the minimap.
		let dragged = Rect { x: 1600.0, ..saved };
		placer.hold();
		assert_eq!(placer.place(dragged, &KeepClear::default(), CANVAS, now), dragged);
		let now = run(&mut placer, dragged, now, (0..80).map(|_| read(MOVING, game)));
		let placement = placer.placement();
		assert!(placement.held);
		assert_eq!(placement.rect, dragged);

		// A slide replaces the game: it looks again, and leaves the text alone.
		let mut now = now;
		while placer.placement().held {
			now = run(&mut placer, dragged, now, [read(STILL, slide)]);
			assert!(now.saturating_duration_since(start) < Duration::from_secs(60));
		}
		let now = run(&mut placer, dragged, now, (0..8).map(|_| read(STILL, slide)));
		let placement = placer.placement();
		assert!(!placement.held);
		// Its own place covers no text, so nothing is clearly better.
		assert_eq!(placement.slot, None);
		assert_eq!(placement.rect, dragged);
		let _ = now;
	}

	/// A move by hand while it is still looking drops the choice it was about
	/// to make: it stays where it was put, over the minimap, as the same game
	/// plays on.
	#[test]
	fn a_move_by_hand_while_it_looks_drops_the_choice() {
		let start = Instant::now();
		let mut placer = Placer::new(start);
		let saved = camera();
		let now = run(&mut placer, saved, start, (0..2).map(|_| read(MOVING, game)));
		assert_eq!(placer.placement().slot, None, "still looking");
		// Moved by hand: only the caller can say so.
		placer.hold();
		let now = run(&mut placer, saved, now, (0..40).map(|_| read(MOVING, game)));
		let placement = placer.placement();
		assert!(placement.held);
		assert_eq!(placement.slot, None);
		assert_eq!(placement.rect, saved);
		let _ = now;
	}

	/// An undo back to where it was saved before a move by hand puts back
	/// where it was meant to be then, the slot it had glided to, at once,
	/// though moves since held it; a redo holds it again. An undo to anywhere
	/// else holds, as does one with no move by hand to undo.
	#[test]
	fn an_undo_of_a_move_by_hand_puts_back_its_place() {
		let start = Instant::now();
		let mut placer = Placer::new(start);
		let saved = camera();
		let keep = KeepClear::default();
		let now = run(&mut placer, saved, start, (0..6).map(|_| read(MOVING, game)));
		let now = now.saturating_add(keep.glide);
		let to = Slot::TopLeft.rect(288.0, 288.0, CANVAS).unwrap();
		assert_eq!(placer.place(saved, &keep, CANVAS, now), to);

		// Dragged over the minimap, then on, a move each.
		let first = Rect { x: 1000.0, ..saved };
		let second = Rect { x: 1600.0, ..saved };
		for dragged in [first, second] {
			placer.hold();
			assert_eq!(placer.place(dragged, &keep, CANVAS, now), dragged);
		}
		let now = run(&mut placer, second, now, (0..8).map(|_| read(MOVING, game)));

		placer.undone(first, now);
		assert_eq!(
			placer.place(first, &keep, CANVAS, now),
			first,
			"back one move, it holds"
		);
		assert!(placer.placement().held);
		placer.undone(saved, now);
		assert_eq!(placer.place(saved, &keep, CANVAS, now), to, "back in its slot, at once");
		let placement = placer.placement();
		assert_eq!((placement.slot, placement.held), (Some(Slot::TopLeft), false));
		let now = run(&mut placer, saved, now, (0..40).map(|_| read(MOVING, game)));
		assert_eq!(placer.placement().rect, to, "it stays as the game plays on");

		// Redone, it holds; undone again, it is back.
		placer.hold();
		assert_eq!(placer.place(first, &keep, CANVAS, now), first);
		assert!(placer.placement().held);
		placer.undone(saved, now);
		assert_eq!(placer.place(saved, &keep, CANVAS, now), to);

		// An undo of an edit that held nothing holds.
		placer.undone(first, now);
		assert_eq!(placer.place(first, &keep, CANVAS, now), first);
		assert!(placer.placement().held);
	}

	/// A rect saved elsewhere is no move by hand unless the caller says so,
	/// as a resize from the top left saves one: it stays in its corner.
	#[test]
	fn a_rect_saved_elsewhere_is_no_move_by_hand() {
		let start = Instant::now();
		let mut placer = Placer::new(start);
		let saved = camera();
		let now = run(&mut placer, saved, start, (0..6).map(|_| read(MOVING, game)));
		assert_eq!(placer.placement().slot, Some(Slot::TopLeft));
		let resized = Rect {
			x: 1600.0,
			y: 80.0,
			width: 262.0,
			height: 262.0,
		};
		let now = run(&mut placer, resized, now, (0..8).map(|_| read(MOVING, game)));
		let placement = placer.placement();
		assert!(!placement.held);
		assert_eq!(placement.slot, Some(Slot::TopLeft));
		let _ = now;
	}

	/// A resize is no move by hand: it stays in its corner at its new size.
	#[test]
	fn a_resize_keeps_its_place() {
		let start = Instant::now();
		let mut placer = Placer::new(start);
		let saved = camera();
		let now = run(&mut placer, saved, start, (0..6).map(|_| read(MOVING, game)));
		assert_eq!(placer.placement().slot, Some(Slot::TopLeft));
		let resized = Rect {
			width: 200.0,
			height: 200.0,
			..saved
		};
		let now = run(&mut placer, resized, now, (0..8).map(|_| read(MOVING, game)));
		let placement = placer.placement();
		assert!(!placement.held);
		assert_eq!(placement.slot, Some(Slot::TopLeft));
		assert_eq!(Some(placement.rect), Slot::TopLeft.rect(200.0, 200.0, CANVAS));
		let _ = now;
	}

	/// The content changing kind is a new choice: a camera in the clear
	/// corner of a slide moves when a game puts its minimap there.
	#[test]
	fn new_content_is_a_new_choice() {
		let start = Instant::now();
		let mut placer = Placer::new(start);
		let saved = camera();
		let now = run(&mut placer, saved, start, (0..8).map(|_| read(STILL, slide)));
		assert_eq!(placer.placement().slot, None, "the top right is clear of text");
		let mut now = now;
		for _ in 0..16 {
			now = run(&mut placer, saved, now, [read(MOVING, game)]);
			if placer.placement().slot.is_some() {
				break;
			}
		}
		assert_eq!(placer.placement().slot, Some(Slot::TopLeft));
		assert!(
			now.saturating_duration_since(start) < Duration::from_secs(2) + CHANGING + SETTLE + INTERVAL * 4,
			"soon after the change, not the worst-place wait"
		);
	}

	/// What it is told not to stay clear of does not matter to it.
	#[test]
	fn a_signal_switched_off_does_not_matter() {
		let start = Instant::now();
		let mut placer = Placer::new(start);
		let saved = camera();
		let keep = KeepClear {
			hud: false,
			text: false,
			..KeepClear::default()
		};
		let mut now = start;
		for _ in 0..8 {
			placer.see(&read(MOVING, game), now);
			placer.place(saved, &keep, CANVAS, now);
			now = now.saturating_add(INTERVAL);
		}
		assert_eq!(placer.placement().slot, None);
	}

	/// Going on again is a new choice too, made from what it sees then.
	#[test]
	fn the_scene_going_on_again_looks_again() {
		let start = Instant::now();
		let mut placer = Placer::new(start);
		let saved = camera();
		placer.shown(Duration::from_secs(5), start);
		let now = run(&mut placer, saved, start, (0..8).map(|_| read(STILL, slide)));
		assert_eq!(placer.placement().slot, None);
		placer.shown(Duration::ZERO, now);
		run(&mut placer, saved, now, (0..6).map(|_| read(MOVING, game)));
		assert_eq!(placer.placement().slot, Some(Slot::TopLeft));
	}
}
