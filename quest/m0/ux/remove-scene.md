# [S] Removing a scene

## Goal

A streamer finds how to remove a scene without knowing to right-click, and can
remove any scene, the one on program included. Removing one on program takes
the next scene first. Removing the show's last scene empties it instead,
leaving one empty scene and the quick start. Removing asks first when the
show is live or the scene holds more than one source; otherwise it removes at
once, and undo brings it back.

## Plan

Found 2026-11-25 trying the first launch: **Remove** sits only in a scene
pad's right-click menu (`ui/shell.rs`, from
[moq-dev/studio#171](https://github.com/moq-dev/studio/pull/171)) and is
disabled for the scene on program, so a one-scene show could never remove it.

Decided 2026-11-25 with the maintainer (/quest-plan):

- **Which scenes:** ✅ any scene, taking the next one first if it is on program
  (rejected: keeping the program scene locked).
- **When to ask:** ✅ a "Remove scene X?" prompt while live or when the scene
  holds more than one source; otherwise remove at once, undoable (rejected:
  never asking; always asking).
- **The last scene:** ✅ removing it empties it to one empty scene (rejected:
  refusing, with "A show needs one scene").
- **Finding it:** ✅ a hovered pad shows a small ⋯ button that opens the same
  menu as a right-click, and Delete or Backspace removes a focused pad
  (rejected: right-click only; a trash button on hover, one stray click from
  the prompt).

Add or update `just shots` views for the hovered pad and the prompt.
