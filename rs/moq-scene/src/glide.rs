//! A glide between two arrangements of one run of items: a
//! [`Transition::Move`](crate::Transition::Move) within a scene, such as its
//! slots reflowing, on the program clock.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::time::Duration;

use kio::clock::Instant;

use crate::collection::{Ease, Filter, Item, Rect, mix};

/// How long a reflow takes.
pub(crate) const REFLOW: Duration = Duration::from_millis(400);

/// A run of items shown as their arrangement changes: once the caller's
/// `K`, naming what moves the items, changes, they glide from where they
/// were shown to where they now are, as a move transition takes them; items
/// it does not move show as they are at once.
pub(crate) struct Glide<K> {
	duration: Duration,
	/// The arrangement shown, or `None` before the first.
	key: Option<K>,
	/// What was shown when the glide under way started, and when.
	from: Option<(Vec<Item>, Instant)>,
	/// What the last [`show`](Self::show) showed.
	shown: Vec<Item>,
	/// Where the last [`show`](Self::show) drew each item it was given.
	placed: Vec<Item>,
}

impl<K: PartialEq> Glide<K> {
	/// A glide taking `duration`, from the second arrangement shown on.
	pub(crate) fn new(duration: Duration) -> Self {
		Self {
			duration,
			key: None,
			from: None,
			shown: Vec::new(),
			placed: Vec::new(),
		}
	}

	/// Each item the last [`show`](Self::show) was given, in its order, where
	/// it was drawn: part way along its glide while one is under way.
	pub(crate) fn placed(&self) -> &[Item] {
		&self.placed
	}

	/// The items to draw at `now` for `to`, arranged as `key` names: `to`
	/// itself, unless `key` changed within the last glide's time, when they
	/// are part way there from what was shown, as [`between`] mixes them.
	/// The first arrangement shows at once.
	pub(crate) fn show(&mut self, key: K, to: &[Item], now: Instant) -> &[Item] {
		if self.key.as_ref() != Some(&key) {
			if self.key.is_some() {
				self.from = Some((std::mem::take(&mut self.shown), now));
			}
			self.key = Some(key);
		}
		let progress = self.from.as_ref().and_then(|(_, start)| {
			let linear = now.saturating_duration_since(*start).as_secs_f32() / self.duration.as_secs_f32();
			(linear < 1.0).then(|| Ease::InOut.apply(linear))
		});
		let same = |a: &Item, b: &Item| a.source == b.source;
		(self.shown, self.placed) = match (progress, &self.from) {
			(Some(progress), Some((from, _))) => (between(from, to, progress, same), placed(from, to, progress, same)),
			_ => {
				self.from = None;
				(to.to_vec(), to.to_vec())
			}
		};
		&self.shown
	}
}

/// `progress` of the way from `from` to `to`, two arrangements of the same
/// run, as a move transition draws them: each visible item of `to` paired by
/// `same` with the first visible item of `from` no earlier one took glides
/// from its rect, crop, and shape there to its own, its filters' settings
/// mixing; the rest of `from` fade out and the rest of `to` fade in where
/// they are. They stack as `from` does until halfway, then as `to` does.
/// Only visible items are returned.
pub(crate) fn between(from: &[Item], to: &[Item], progress: f32, same: impl Fn(&Item, &Item) -> bool) -> Vec<Item> {
	let (left, entered) = pair(from, to, same);
	let slots: Vec<(Option<usize>, Option<usize>)> = if progress < 0.5 {
		stack(from, &left, to, &entered)
	} else {
		let slots = stack(to, &entered, from, &left);
		slots.into_iter().map(|(to, from)| (from, to)).collect()
	};
	let faded = |item: &Item, opacity: f32| {
		let mut item = item.clone();
		item.filters.push(Filter::Opacity { amount: opacity });
		item
	};
	slots
		.into_iter()
		.map(|slot| match slot {
			(Some(at), None) => faded(&from[at], 1.0 - progress),
			(None, Some(index)) => faded(&to[index], progress),
			(Some(at), Some(index)) => {
				let (old, new) = (&from[at], &to[index]);
				let look = match mixed(old, new, progress) {
					Some(filters) => Item { filters, ..new.clone() },
					// A look that cannot be mixed changes as the stacking does.
					None if progress < 0.5 => Item {
						keyframes: new.keyframes.clone(),
						..old.clone()
					},
					None => new.clone(),
				};
				Item {
					rect: toward(&old.rect, &new.rect, progress),
					crop: old.crop.toward(&new.crop, progress),
					shape: old.shape.toward(&new.shape, progress),
					..look
				}
			}
			(None, None) => unreachable!("a slot holds an item"),
		})
		.collect()
}

/// Each item of `to` where [`between`] draws it at `progress`: one paired
/// with an item of `from` part way from its place there, the rest where
/// they are.
fn placed(from: &[Item], to: &[Item], progress: f32, same: impl Fn(&Item, &Item) -> bool) -> Vec<Item> {
	let (_, entered) = pair(from, to, same);
	let paired = to.iter().zip(entered);
	paired
		.map(|(new, at)| match at {
			Some(at) => Item {
				rect: toward(&from[at].rect, &new.rect, progress),
				crop: from[at].crop.toward(&new.crop, progress),
				shape: from[at].shape.toward(&new.shape, progress),
				..new.clone()
			},
			None => new.clone(),
		})
		.collect()
}

/// `progress` of the way from `from` to `to`.
fn toward(from: &Rect, to: &Rect, progress: f32) -> Rect {
	Rect {
		x: mix(from.x, to.x, progress),
		y: mix(from.y, to.y, progress),
		width: mix(from.width, to.width, progress),
		height: mix(from.height, to.height, progress),
	}
}

/// Which item of `entering` each visible item of `leaving` pairs with, and
/// the other way: each visible item of `entering` takes the first visible
/// item of `leaving` that `same` matches and no earlier one took.
pub(crate) fn pair<A: Borrow<Item>, B: Borrow<Item>>(
	leaving: &[A],
	entering: &[B],
	same: impl Fn(&Item, &Item) -> bool,
) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
	let mut left = vec![None; leaving.len()];
	let mut entered = vec![None; entering.len()];
	for (index, item) in entering.iter().enumerate() {
		let item = item.borrow();
		if !item.visible {
			continue;
		}
		let pair = (0..leaving.len()).find(|&at| {
			let old = leaving[at].borrow();
			left[at].is_none() && old.visible && same(old, item)
		});
		if let Some(at) = pair {
			left[at] = Some(index);
			entered[index] = Some(at);
		}
	}
	(left, entered)
}

/// `old`'s filters `progress` of the way to `new`'s, or `None` if their blend
/// modes differ or their filters do not pair up by kind, in order, as
/// [`Filter::toward`] mixes them. An opacity one has past the other's last
/// filter, as a glide under way fades an item, mixes with full opacity.
pub(crate) fn mixed(old: &Item, new: &Item, progress: f32) -> Option<Vec<Filter>> {
	if old.blend != new.blend {
		return None;
	}
	let opaque = Filter::Opacity { amount: 1.0 };
	let padded = |filters: &[Filter], to: usize| -> Option<Vec<Filter>> {
		let mut filters = filters.to_vec();
		match to.checked_sub(filters.len())? {
			0 => {}
			1 => filters.push(opaque.clone()),
			_ => return None,
		}
		Some(filters)
	};
	let len = old.filters.len().max(new.filters.len());
	let (from, to) = (padded(&old.filters, len)?, padded(&new.filters, len)?);
	if old.filters.len() != new.filters.len() {
		let longer = if old.filters.len() > new.filters.len() {
			&from
		} else {
			&to
		};
		if !matches!(longer.last(), Some(Filter::Opacity { .. })) {
			return None;
		}
	}
	let pairs = from.iter().zip(&to);
	pairs.map(|(old, new)| old.toward(new, progress)).collect()
}

/// The order to draw a move's visible items in, back to front, as `lead`
/// stacks them: each slot is an item of `lead`, of `other`, or a pair of
/// both, as `lead_pairs` and `other_pairs` pair them. An unpaired item of
/// `other` goes just over the paired item it was over in `other`, or under
/// every slot if it was under every paired item.
pub(crate) fn stack<A: Borrow<Item>, B: Borrow<Item>>(
	lead: &[A],
	lead_pairs: &[Option<usize>],
	other: &[B],
	other_pairs: &[Option<usize>],
) -> Vec<(Option<usize>, Option<usize>)> {
	let mut over: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
	let mut under = None;
	for index in (0..other.len()).filter(|&index| other[index].borrow().visible) {
		match other_pairs[index] {
			Some(_) => under = Some(index),
			None => over.entry(under).or_default().push(index),
		}
	}
	let mut unpaired = |under: Option<usize>| {
		let items = over.remove(&under).unwrap_or_default();
		items.into_iter().map(|index| (None, Some(index)))
	};

	let mut slots: Vec<_> = unpaired(None).collect();
	for index in (0..lead.len()).filter(|&index| lead[index].borrow().visible) {
		let pair = lead_pairs[index];
		slots.push((Some(index), pair));
		if pair.is_some() {
			slots.extend(unpaired(pair));
		}
	}
	slots
}

#[cfg(test)]
mod tests {
	use super::*;

	fn at(source: &str, x: f32) -> Item {
		Item::new(
			source,
			Rect {
				x,
				y: 0.0,
				width: 100.0,
				height: 100.0,
			},
		)
	}

	fn opacity(item: &Item) -> f32 {
		match item.filters.last() {
			Some(Filter::Opacity { amount }) => *amount,
			_ => 1.0,
		}
	}

	#[test]
	fn a_joining_item_fades_in_while_the_others_glide() {
		let from = vec![at("main", 0.0), at("cam", 500.0)];
		let to = vec![at("main", 0.0), at("cam", 300.0), at("guest", 600.0)];
		let half = between(&from, &to, 0.5, |a, b| a.source == b.source);
		assert_eq!(half.len(), 3);
		assert_eq!(half[1].rect.x, 400.0);
		assert_eq!(half[2].source, "guest");
		assert_eq!(opacity(&half[2]), 0.5);
		assert_eq!(opacity(&half[0]), 1.0);
	}

	#[test]
	fn a_leaving_item_fades_out_where_it_was() {
		let from = vec![at("cam", 0.0), at("guest", 600.0)];
		let to = vec![at("cam", 300.0)];
		let quarter = between(&from, &to, 0.25, |a, b| a.source == b.source);
		let guest = quarter.iter().find(|item| item.source == "guest").unwrap();
		assert_eq!(guest.rect.x, 600.0);
		assert_eq!(opacity(guest), 0.75);
	}

	/// A glide that starts while one is under way starts from what it showed,
	/// so nothing jumps: a guest half faded in carries on from half.
	#[test]
	fn a_glide_under_way_restarts_from_what_it_showed() {
		let start = kio::clock::now();
		let mut glide = Glide::new(Duration::from_millis(400));
		let one = vec![at("cam", 0.0)];
		let two = vec![at("cam", 0.0), at("guest", 600.0)];
		let three = vec![at("cam", 200.0), at("guest", 600.0)];
		assert_eq!(glide.show(1, &one, start), one.as_slice());
		// It starts with the first draw that sees the new arrangement.
		let joined = glide.show(2, &two, start).to_vec();
		assert_eq!(opacity(&joined[1]), 0.0);
		let halfway = start.saturating_add(Duration::from_millis(200));
		let half = glide.show(2, &two, halfway).to_vec();
		assert_eq!(opacity(&half[1]), 0.5);
		assert_eq!(glide.placed(), two.as_slice(), "nothing moves, so each is where it is");
		// The arrangement changes again at once: the guest goes on from half.
		let again = glide.show(3, &three, halfway).to_vec();
		assert_eq!(opacity(&again[1]), 0.5);
		assert_eq!(again[0].rect.x, 0.0);
		let quarter = halfway.saturating_add(Duration::from_millis(100));
		let drawn = glide
			.show(3, &three, quarter)
			.iter()
			.find(|item| item.source == "cam")
			.unwrap()
			.rect;
		assert_eq!(
			glide.placed()[0].rect,
			drawn,
			"placed where it is drawn, in the order given"
		);
		assert!(drawn.x > 0.0 && drawn.x < 200.0);
		let later = glide
			.show(3, &three, halfway.saturating_add(Duration::from_millis(400)))
			.to_vec();
		assert_eq!(later, three);
	}

	#[test]
	fn the_first_arrangement_shows_at_once() {
		let mut glide = Glide::new(REFLOW);
		let items = vec![at("cam", 0.0)];
		assert_eq!(glide.show((), &items, kio::clock::now()), items.as_slice());
	}
}
