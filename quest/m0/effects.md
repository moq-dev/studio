# [M] Video effects

## Goal

Sources and scene items in `moq-studio` take a curated set of video effects:
color correction, `.cube` LUTs, blur, crop and masks, chroma and luma key,
and blend modes. They render the same in every composition host. ML effects
such as background removal are offered too, applied before the source
publishes.

## Plan

Write the effects in WGSL to the meanings the CSS Filter Effects and
Compositing specs define (color matrices, blur, drop-shadow, blend modes), so
an effect means the same thing as the CSS overlays use. The layout track
carries effect names and parameters, never shader code, since a shader from
the network costs every viewer's GPU and can lose the device. An effect the
browser cannot reproduce, like an ML model, is baked into its source's track
per the [composition epic](/quest/m0/compose/README.md), so ML effects
apply to a source, never to one scene item. Prefer an
existing model crate for ML effects over training one.

Effects extend moq-scene's `Filter`, which a source (`[video.<name>]`) or an
item lists, so they undo, copy, and paste as opacity, the first filter, does.
