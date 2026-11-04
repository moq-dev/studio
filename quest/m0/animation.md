# [M] Keyframes and stingers

## Goal

Any item property in `moq-studio` can be keyframed with easing, so an item
can fly in or a camera can push in, and a scene change can run a stinger: a
video clip played over the program that switches scenes at its cut point.

## Plan

Fade, slide, wipe, and move transitions are drawn by moq-scene's compositor
on the program clock
([moq-dev/studio#33](https://github.com/moq-dev/studio/pull/33)). Animate in
moq-scene on the media clock, so the program, a recording, and a replay match.
Carrying transitions and keyframes in the layout track, for viewers to
compose, waits for the m1 [composition epic](/quest/m1/compose/README.md)
(changed 2026-11-04, maintainer: the program comes first). Overlays animate
through their own CSS. No Lottie runtime for now (decided 2026-10-26): the
open ones either lack text (velato) or add a C++ build (ThorVG). Keyframes and
stinger settings change through moq-scene's `Edit`, so they undo like any edit.

A stinger clip is a media source like any other, but no source plays a media
file yet, and a stinger must start its clip as the transition starts or its
cut point lands at random. It waits on a media-file source that plays from
the start on demand, which no quest plans yet.

Open questions for the maintainer:

- What starts an item's keyframes: its scene going on program (and into
  preview, for the preview), a loop on the program clock, or a trigger such
  as a hotkey. Recommended: the time since its scene went on program, playing
  once and holding the last keyframe.
- How a keyframed item edits: a drag in the preview sets the keyframe at the
  preview's time, or the item's resting place, from which keyframes are
  offsets.

## Related

- [Layout track](/quest/m1/compose/layout.md) - where transitions and keyframes travel to viewers in m1
