# [M] Multiple canvases

## Goal

The studio composites several canvases at once from shared sources, such as
a 1920x1080 landscape canvas and a 1080x1920 portrait one. Each canvas has
its own layout per scene, its own preview, and its own outputs. A source
captures or decodes once however many canvases use it.

## Plan

Further canvases, their layouts and top layers, linked scene switching, and a
broadcast per canvas at `<--broadcast>/<name>` landed in
[moq-dev/studio#42](https://github.com/moq-dev/studio/pull/42). What remains:

- Unlinking: a canvas the creator unlinks keeps its own preview and program
  scene, so a scene change no longer cuts it with the others. Transitions are
  per canvas, so an unlinked canvas takes its own.
- Outputs per canvas: a recording, and each destination, picks the canvas it
  carries, as each picks its mix. Only the main canvas records today.
- Health per canvas: the Health panel grades each canvas's broadcast, with
  viewer feedback beneath it, not only the main one's.
- Preview cost: a canvas no panel shows skips drawing its preview, so an
  unseen canvas costs only its program.

Per-canvas cost must grow only with the sources that canvas uses, never with
the whole source table; `compositor::bench` measures it.
