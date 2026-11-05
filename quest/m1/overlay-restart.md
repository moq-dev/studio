# [XS] Overlays restart on air

## Goal

An overlay's CSS timeline restarts each time it goes on program, so a lower
third plays its entrance at every cut, unless the overlay is set to keep
running.

## Plan

From [moq-dev/studio#34](https://github.com/moq-dev/studio/pull/34).

Decided 2026-11-04 with the maintainer (/quest-plan):

- ✅ **A per-overlay `restart` flag, true by default**, like OBS's "refresh
  when the scene becomes active". A clock or ticker overlay turns it off so
  it keeps running across cuts.
  Rejected: always restart (no way to keep a running clock).

Implementation:

- `rs/moq-scene/src/overlay.rs` already starts an overlay's CSS timeline at a
  `start` on the program clock; with `restart`, reset `start` when the item's
  scene goes on program, matching when keyframes start.
- A scene in preview restarts its overlays when it goes into preview, as
  keyframes do, so the preview shows the entrance.
- The flag is in the scene file, the overlay's add form, and `README.md`.
