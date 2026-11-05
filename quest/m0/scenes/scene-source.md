# [M] Scene source

## Goal

A new source kind, `scene`, draws another scene, or `program` (whatever is on
air, including a transition in progress), as a picture that an item places,
scales, crops, and filters like any other. A scene cannot contain itself,
directly or through other scenes; such an edit is refused with an error.

## Plan

Decided 2026-11-05 with the maintainer (/quest-plan); see the
[epic](/quest/m0/scenes/README.md) for the model.

- A nested scene renders at its own size into a texture the compositor
  samples, once per frame however many items draw it, and only while
  something that is drawn reads it.
- `program` reads the frame the program output draws, so a scene drawing it
  shows the same cut on the same frame, with no extra composite.
- Keyframes, stingers, and restarts in a nested scene start when the scene
  that draws it goes on program, as its items would.
- Measure frame time and GPU copies with and without a nested scene.
- `README.md` documents the source kind.
