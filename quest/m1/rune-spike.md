# [M] Rune UI spike

## Goal

A prototype of the Preview and Program monitors built on
[Rune](https://github.com/anomalyco/rune) answers whether the studio's UI
should move from egui to Rune. Its PR reports what it found and recommends
moving or staying.

## Plan

Decided 2026-11-07 with the maintainer (/quest-plan), after choosing
[UI effects](https://github.com/moq-dev/studio/pull/85) on rune-fx inside
egui first, whose experience feeds the verdict:

- ✅ **Prototype the Preview and Program monitors:** live compositor textures,
  drag-to-edit items, and the Cut button. They are the UI's hardest part, so
  they say the most about the rest.
- The report covers frame time against egui on the same scene, docking (what
  replaces egui_dock), sharing the compositor's wgpu textures, how well
  rune-kit's components fit, testing headlessly by rendering PNGs and
  inspecting the element tree, and the cost of a pre-release dependency that
  takes no pull requests. No fixed pass bar: the maintainer decides from the
  report.
- The prototype stays off `main`: the PR deletes this quest and links the
  prototype branch. A move to Rune, if picked, is planned with /quest-plan.
