# [M] Each layer has an identity the studio keeps while it runs

## Goal

Each item of a scene and of the top layer carries a runtime identity: a new
item gets a new one, an edit that changes an item keeps its own, undo and
redo bring back the one it had, and the scene file never saves it. What the
compositor keeps about a layer between frames is matched to its item by
that identity, not by where it sits, so adding, removing, reordering, or
duplicating layers never hands one layer's state to another.

## Plan

Keep out of the way (`rs/moq-scene/src/place.rs`) keeps a placer per layer:
what it saw, the slot it chose, and whether a move by hand holds it. With no
identity on an item, `compositor::follow` matches placers to items by
source, then saved rect, then index, after each edit. Two layers of the same
source at the same rect, as Duplicate makes them, can still trade placers
when a layer is added under both. Auto-layout's glide pairs items by source
for the same reason.

- **The identity:** a counter-made id on `Item`, outside serde, that
  `Item::new`, loading, and every edit that adds an item as a new one
  assign: BUILD's Duplicate (`LayerAction::Duplicate`), paste, a drop on the
  picture (`ui/drop.rs`), an Add, and a guest joining. `set_item` and the
  history keep the id of the item they change, so an undo restores it.
- **Equality:** two collections that differ only in ids are equal, so
  autosave, reload, and every change check behave as now. A reload assigns
  fresh ids, then matches them to the items before by the same rule
  `follow` uses, so a reload of an unchanged file keeps every placer.
- **Users:** the compositor's keepers key by id; `follow` keeps only the
  reload match. The slots' glide (`rs/moq-scene/src/glide.rs`, which pairs
  items by source) pairs by id where both sides have one.
- **Undo across layers:** a placer whose layer leaves its scene, as Keep
  always on top does, stays parked under its id
  while the history can bring the layer back, so the undo puts it back where
  it was, as an undo of switching keep-clear off does now (#194's decision
  11). Today such an undo starts a fresh placer at the saved rect.
- **Tests:** duplicates of one camera keep their own placers across an
  insert under both, a reorder, an undo of a removal, an undo of Keep always
  on top, and a reload of an unchanged file.

Decided 2026-11-17 (maintainer, on
[#194](https://github.com/moq-dev/studio/pull/194#issuecomment-6944247792)):
✅ a runtime identity, as its own quest (rejected: in #194, more matching
heuristics in #194).

## Related

- [A camera that keeps out of the way remembers each app](/quest/m1/auto-place.md) - another user of the placers this keys
