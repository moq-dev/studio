# [M] Keyframes

## Goal

Any item property in `moq-studio` can be keyframed with easing, so an item
can fly in or a camera can push in, and the same animation plays in the
program, a recording, and a replay.

## Plan

Fade, slide, wipe, and move transitions are drawn by moq-scene's compositor
on the program clock
([moq-dev/studio#33](https://github.com/moq-dev/studio/pull/33)). Animate in
moq-scene on the media clock, so the program, a recording, and a replay match.
Carrying keyframes in the layout track, for viewers to compose, waits for the
m1 [composition epic](/quest/m1/compose/README.md) (changed 2026-11-04,
maintainer: the program comes first). Overlays animate through their own CSS.
No Lottie runtime for now (decided 2026-10-26): the open ones either lack text
(velato) or add a C++ build (ThorVG). Keyframes change through moq-scene's
`Edit`, so they undo like any edit.

Decided 2026-11-04 (maintainer):

- **Start:** an item's keyframes run on the time since its scene went on
  program (and into preview, for the preview), play once, and hold the last
  keyframe. Rejected: a loop on the program clock, and a trigger or hotkey.
- **Editing:** a drag in the preview moves the item's resting place, and
  keyframes are offsets from it, so editing needs no timeline scrubber.
  Rejected: setting the keyframe at the preview's time.

Stingers are their own quest, [Stinger transitions](/quest/m0/stinger.md).

## Related

- [Layout track](/quest/m1/compose/layout.md) - where keyframes travel to viewers in m1
