# [M] A camera that keeps out of the way remembers each app, in each format

## Goal

A layer with **Keep out of the way** on knows the app in front: it chooses
again the moment another app comes to the front, rather than waiting to see
the picture change, and the place it chose for an app is remembered per user,
so a known game is right from its first frame. While the show goes out in
landscape and portrait at once, it keeps out of the way in each picture on
its own.

The decision stays on the machine. Nothing is uploaded.

## Plan

The rest of the quest landed: moq-scene's `place` module reads the picture
under the layer on the GPU a few times a second, weighs motion, still edges
(text), and still edges while the rest moves (a HUD), and keeps the layer in
the corner or edge middle that covers the least, gliding there; BUILD shows
what it saw, and the inspector its settings (`keep_clear` in the scene file).
What remains needs moq to report the app in front.

- **When it moves:** a notice that the app in front changed starts a new
  choice at once, as the content changing kind does now
  (`Placer::see` setting `looking`), for the display capture under the
  layer.
- **Memory:** the slot chosen for each app, keyed by the app moq reports,
  kept per user with the hotkeys and snap settings, not in the scene file.
  A remembered slot is where the layer starts when that app comes to the
  front, before it has looked; a move by hand or a new choice updates it.

Decided 2026-11-09 (maintainer, /quest-plan): ✅ local only (rejected: a
hosted decision model asked on each content change, cheap but it uploads
thumbnails of the streamer's screen and its image input is not production
ready; local first with a hosted model as an opt-in tie-breaker). Should the
signals prove too weak, a small on-device model is a later quest, not a
hosted one.

- **In each format:** while the show goes out in landscape and portrait at
  once, a layer keeping out of the way places only its landscape picture;
  its portrait place stays as set by hand or derived when **Both at once**
  was turned on. Placing each picture on its own remains: the analysis reads
  the portrait picture too, chooses among the portrait canvas's slots, and
  writes its `portrait` place through moq-scene's `Item::placed`, with a
  `Glide` per view (`View::reflows` already is one). A layer shown in one
  format only is placed in that one alone.

Decided 2026-11-18 (maintainer, [moq-dev/studio#200](https://github.com/moq-dev/studio/pull/200#issuecomment-6994130042)):
✅ landscape only in that PR, each format here (rejected: a placer per
format in that PR).

## Required

- [moq-video's foreground app is pinned](/quest/m1/pin-capture-foreground.md) - when the app in front changes, and the key its slot is remembered under

## Related

- [Living with it](/quest/m1/auto-place-check.md) - whether it moves too often, judged on real streams
