# [XS] Premultiplied source sampling

## Goal

A scaled source with transparency, such as a PNG logo, draws without a dark
or light fringe at its edges, because the compositor samples it
premultiplied.

## Plan

From [moq-dev/studio#17](https://github.com/moq-dev/studio/pull/17), raised
by CodeRabbit on `rs/moq-scene/src/effect.wgsl`. It predates that PR.

- Source pictures are sampled bilinearly straight (not premultiplied), so a
  transparent texel's color bleeds into its neighbours when scaled.
  Premultiply before sampling: either upload premultiplied, or sample with
  the effect pass's premultiplied flag, whichever adds no GPU copy.
- A compositor test scales a transparent picture with a colored transparent
  region and checks that edge texels carry no color from it.
- Measure frame time before and after.
