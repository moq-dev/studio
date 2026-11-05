# [L] Published scenes

## Goal

Any scene can be published: ticked **Publish**, it goes live as its own
broadcast at `<--broadcast>/<scene>`, at its own size, with the audio scene it
picks (the one on program by default). This replaces canvases: further
canvases, their per-scene layouts, and the canvas picker are removed, and a
portrait broadcast is a published portrait scene that draws the program
through a [scene source](/quest/m0/scenes/scene-source.md).

## Plan

Decided 2026-11-05 with the maintainer (/quest-plan); see the
[epic](/quest/m0/scenes/README.md).

- Every scene has a size, defaulting to the program's; the switcher offers
  only scenes of the program's size.
- The program keeps one switcher at `<--broadcast>`. A published scene is
  always live and never switched.
- Each published scene gets the program's two sizes (full and half) and the
  bandwidth grant shares across all of them, as further canvases did.
- Carried from [moq-dev/studio#54](https://github.com/moq-dev/studio/pull/54):
  Health grades each published broadcast with its viewer feedback; a
  recording picks a picture scene and an audio scene; a scene no panel shows
  and no output reads skips drawing.
- Remove `[canvases]` from the scene file with no migration; nothing is
  released. `README.md` documents published scenes.

## Required

- [Scene source](/quest/m0/scenes/scene-source.md) - a portrait scene follows the program through it
