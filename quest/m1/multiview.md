# [M] A multiview window

## Goal

A second window, for another monitor, shows every scene and chosen sources
in a grid of live pictures with their names, the scene on air framed red and
the cued one amber, and a clock. A click cues or takes, as a scene pad does.
See the `multiview` frame of
[`mock/later.html`](/quest/m0/ux/mock/later.html).

## Plan

It is the operator's view a broadcast studio expects beside Studio mode.

- The tiles are scene thumbnails (`moq_scene::Thumbnail`) at a larger size,
  and the program's own rate for the tiles on air and cued, a lower one for
  the rest. Measure the cost of a 4x4 grid.
- The grid is uniform, since the main window already shows the live picture
  large. A source the live picture shows carries a red dot, never a frame:
  the red frame is the scene on air alone.
- It goes full screen on the monitor it is on, and its place is remembered
  per user.
- It is the studio's own window, so
  [the privacy shield](/quest/m1/privacy-shield.md) keeps it out of capture.

Human check: on a real second monitor, at a distance.

## Required

- [Editing off air, and Studio mode](/quest/m0/ux/studio-mode.md) - cue, take, and the amber and red it shows
