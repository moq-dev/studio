# [M] Animation and transitions

## Goal

Scene changes in `moq-studio` use transitions beyond a cut and fade: move
(items glide between their positions in the two scenes), slide, wipe, and
stinger (a video clip with a cut point). Any item property can be keyframed
with easing, so an item can fly in or a camera can push in.

## Plan

Move is OBS's most-installed plugin (Move Transition), and stingers are
standard in broadcast. Animate in moq-scene on the media clock, so the program, a recording, and a
replay match. Carrying transitions and keyframes in the layout track, for
viewers to compose, waits for the m1
[composition epic](/quest/m1/compose/README.md) (changed 2026-11-04,
maintainer: the program comes first).
Overlays animate through their own CSS. No Lottie runtime for now (decided
2026-10-26): the open ones either lack text (velato) or add a C++ build
(ThorVG). A stinger clip is a media source like any other. Transitions and
keyframes change through moq-scene's `Edit`, so they undo like any edit.

## Related

- [Layout track](/quest/m1/compose/layout.md) - where transitions and keyframes travel to viewers in m1
