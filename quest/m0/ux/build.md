# [L] The BUILD page

## Goal

The BUILD page of the [mock-up](/quest/m0/ux/mock/shell-build.webp) replaces
the Items, Sources, Filters, and Keyframes panels:

- **Layers** on the left list what the scene draws, front to back, with the
  always-on-top layers in a group of their own. Each row has an icon for its
  kind, its name, what it shows in words, and eye and lock toggles. Rows drag
  to reorder.
- **The picture** is edited directly. A selected layer has handles and a
  small toolbar on it (crop, fit, fill, shape, cut out, more), and a right
  click anywhere offers what applies there.
- **The inspector** on the right follows the selection: place, shape, look
  (background removal, color grade, filters, blend), and an Animate tab that
  holds the layer's keyframes.
- **Linked or independent is visible:** a layer that shows a source other
  scenes also show says so, with how many, and offers to make it
  independent. Duplicating a scene asks which.

Undo and redo are the title bar's icons, and the history is a list under the
undo icon. The scene graph moves to Settings, Advanced.

## Plan

The words change with the place: "item" becomes "layer" and "top layer"
becomes "always on top" in the UI and in `README.md`. The scene file's keys
do not change.

- A source nothing shows no longer needs a panel of its own: Layers lists
  the scene's layers, and unused sources appear under Add, where a source is
  also removed for good.
- "Make independent" is today's clone (`kind = "clone"`). OBS users lose
  work to the opposite default, where a duplicate silently shares its
  sources, so say which one a duplicate will be before it happens.
- Every status a source has today (live, stalled, ended, failed with its
  reason, GPU or CPU for a web page, the delay a broadcast plays behind)
  shows on its layer row as an icon with the words on hover, and Play, Stop,
  and Reload are in its inspector.
- Snapping, the grid, nudging, aspect lock, and copy and paste keep working
  as `interact.rs` and `snap.rs` have them. Add the right-click menus and the
  on-picture toolbar; there is no context menu anywhere today.
- Errors that were one status-bar line (`Panels::message`) appear where they
  happened: on the layer, the field, or the pad.
- **Interact** with a web page becomes a mode of the selected layer, entered
  from its toolbar and left with Escape.

Human check: feel of dragging, resizing, and reordering on a real machine,
and whether the inspector's grouping is where a streamer looks first.

## Required

- [One live picture](/quest/m0/ux/live.md) - the frame, the stage, and the page this fills
