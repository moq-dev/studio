# [L] A camera that keeps out of the way

## Goal

A layer with **Keep out of the way** on moves itself so it does not cover what
matters in the picture under it: a game's minimap, a slide's text, the window
the streamer is working in. It chooses among a few slots along the edges,
moves only when the content changes, and glides there. The `autoplace` frame
of [`mock/later.html`](/quest/m0/ux/mock/later.html) shows the slots, what it
saw, and the move.

The decision is made on the machine. Nothing is uploaded.

## Plan

Decided 2026-11-09 (maintainer, /quest-plan): ✅ local only (rejected: a
hosted decision model asked on each content change, cheap but it uploads
thumbnails of the streamer's screen and its image input is not production
ready; local first with a hosted model as an opt-in tie-breaker).

- **What it reads:** the composited picture under the layer, scaled far down
  on the GPU a few times a second: where it moves, where it is dense with
  edges and text, and what stays put while the rest moves, which is a HUD.
  The inspector says which of these it stays clear of, and each can be
  switched off. It reads neither the pointer nor the focused window, since
  moq reports neither a pointer nor a window's place on its display.
- **What it decides:** the slot whose area matters least, among the corners
  and edge middles that fit the layer, inside the canvas's margins.
- **When it moves:** when the scene changes, when the content under it
  changes kind (another layout, or another app in front where
  [moq reports it](/quest/m1/pin-capture-foreground.md)), or when its slot has been the
  worst for a sustained time. Never on a single busy frame, and never while
  the streamer is dragging it. A manual move wins and holds until the content
  changes.
- **Memory:** the slot chosen for an app is remembered per user, so a known
  game is right from its first frame.
- **Where it lives:** the analysis and the choice are UI-free, in moq-scene,
  tested on recorded frames with a paused clock. The move is the item's
  rect animated as a Move transition eases.

Measure, per AGENTS.md: the frame time and GPU copies the analysis adds, and
that the encoder's load does not move. Should the signals prove too weak, a
small on-device model is a later quest, not a hosted one.

Human check: living with it for a few streams. Whether it moves too often is
a judgment no test makes.

## Required

- [The BUILD page](/quest/m0/ux/build.md) - the inspector's switch and the layer it moves

## Related

- [moq-video's foreground app is pinned](/quest/m1/pin-capture-foreground.md) - when the app in front changes, and the key its slot is remembered under
- [Scenes that lay themselves out](/quest/m1/auto-layout.md) - slots chosen by role rather than by content
