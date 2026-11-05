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
  Premultiply before interpolation: upload premultiplied, or premultiply each
  tap in the shader, whichever adds no GPU copy. The effect pass's
  `premultiplied` flag is not the fix: `fetch()` in `effect.wgsl` interpolates
  first and `straight()` divides afterwards, so the flag only describes the
  input.
- A compositor test scales a transparent picture with a colored transparent
  region and checks that edge texels carry no color from it.
- Measure frame time before and after.
