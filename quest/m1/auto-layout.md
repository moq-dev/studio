# [L] Scenes that lay themselves out

## Goal

A scene can be built from slots instead of fixed rectangles: a main slot, a
camera slot, and a row or grid of guest slots. A slot holds whatever fills
its role, and the scene reflows with a glide when a guest joins, a guest
leaves, or a source goes away. The `autolayout` frame of
[`mock/later.html`](/quest/m0/ux/mock/later.html) shows one scene before and
after a second guest joins.

A show names its roles once (which camera is the main camera, which window is
the main picture), so a scene or a template built from roles survives a
change of hardware.

## Plan

- A slot layout is a new way to place a scene's items, beside today's `rect`,
  in moq-scene and the scene file. A scene mixes both: a logo stays where it
  was put while the guests reflow. Settle the file's shape here, keeping it
  hand-editable.
- Roles map names to sources per show. An unfilled role draws nothing and
  its slot closes up.
- A guest who joins fills the next guest slot of the scene on air only if
  the show says guests appear on their own; otherwise they wait in Add, as
  today.
- The reflow is a Move transition between the two arrangements, on the
  program clock, so the stream and a recording show the same frames.
- BUILD shows slots as outlines with their role names, and dragging a layer
  onto a slot assigns it.

Human check: whether the reflow reads as intended rather than as a glitch,
with real guests joining mid-show.

## Required

- [The BUILD page](/quest/m0/ux/build.md) - where slots are drawn and filled

## Related

- [First run](/quest/m0/ux/first-run.md) - its starting layouts become role-based templates
