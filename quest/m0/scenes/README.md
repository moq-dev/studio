# [M] Composable scenes

## Goal

Scenes form a graph. A scene can draw another scene, or the live program, as
a source. Any scene can be published as its own broadcast at its own size, and
the old audio mixes are audio-only scenes that nest the same way. This
replaces separate canvases: a portrait broadcast is a published 1080x1920
scene that draws the program, cropped, with its own overlays.

This epic's own work is an end-to-end test: one studio publishes the program
at `<--broadcast>` and a portrait scene drawing it at
`<--broadcast>/<scene>`, each carrying its picked audio scene, and a viewer of
each sees and hears the same cut at the same moment.

## Plan

Decided 2026-11-05 with the maintainer (/quest-plan), replacing the canvases
of [moq-dev/studio#42](https://github.com/moq-dev/studio/pull/42) and closing
[moq-dev/studio#54](https://github.com/moq-dev/studio/pull/54):

- ✅ **Published scenes + scene source** rather than canvases with a layout
  per scene and linked or unlinked switching. One concept (a scene) instead of
  two, and no unlinking. Rejected: published scenes that never follow cuts;
  keeping canvases.
- ✅ **A scene source draws any scene, or `program`** (whatever is on air,
  transitions included), scaled and cropped like any picture. Rejected: a
  program-only source.
- ✅ **The program stays one switcher at `<--broadcast>`**: Preview, Program
  and Cut work as today. Other published scenes are always live at
  `<--broadcast>/<scene>` and follow cuts only through a scene source.
  Rejected: every published scene switchable on its own.
- ✅ **Every scene has a size**, defaulting to the program's. The switcher
  offers only scenes of the program's size.
- ✅ **Mixes become audio scenes** that hold only sounds and nest, so a
  "Mics" scene can feed "Stream" and "Headphones". Audio stays independent of
  cuts, as today. Each published output pairs a picture scene with an audio
  scene, defaulting to the one on program. Rejected: scenes carrying their own
  audio, so a cut switches audio; keeping `[mixes]` separate.
- ✅ **UI:** the Scenes panel lists picture and audio scenes, tagged by kind.
  The Audio panel stays a mixer (fader, pan, meter, and sends to audio scenes)
  for live work, as pro switchers do. A graph view of the same data is
  [its own quest](/quest/m1/scene-graph.md).
- Nothing is released, so the scene file breaks freely: `[canvases]` and
  `[mixes]` go away, with no migration.

Carried over from #54: Health grades each published broadcast; a recording
picks a picture scene and an audio scene; a scene no panel shows and no output
reads skips drawing. Per-scene cost grows only with what that scene draws;
`compositor::bench` measures it.

Changed 2026-11-07 with the maintainer, on
[moq-dev/studio#77](https://github.com/moq-dev/studio/pull/77): pan left
this epic for [its own quest](/quest/m0/audio-pan.md), since it waits on
moq-audio, so it no longer holds up this end-to-end test.
