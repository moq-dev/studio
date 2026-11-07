# [M] The UI keeps its frame rate while the program renders

## Goal

The studio's UI draws its frames without waiting on the program's render, so
the editor stays as smooth as the display allows while the studio publishes
or records.

## Plan

From [moq-dev/studio#85](https://github.com/moq-dev/studio/pull/85), picked
2026-11-07 (maintainer away under /afk, recommended option), m0 by the rule
that fixes join m0.

#85 measured the UI at about 25 fps (40 ms a frame) with effects Full,
Reduced, and Off alike, while the program rendered in about 19 ms, on
`nixos`'s RTX 3070 Ti under Xvfb at 1440x900. The UI's own CPU work was
under 2 ms. Its unprofiled guess is that the UI waits on `engine.state`'s
lock while the program renders.

- Profile first, on a real display as well as Xvfb, since Xvfb's
  presentation may account for some of it. Report frame time, where the UI
  thread waits, and GPU queue contention between the UI and the compositor.
- Fix the cause found, such as holding `engine.state` only to copy what a
  frame draws, or keeping the compositor's submit off the UI's path. If the
  wait is only Xvfb's, record that in the PR and delete this quest.
- Show the program's render time and encoder load are unchanged.
