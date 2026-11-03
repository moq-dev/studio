# [M] ML video effects

## Goal

Sources in `moq-studio` offer ML video effects, starting with background
removal (a person cut out of a camera), applied to the source's picture before
it is drawn or published, so every viewer sees the same cut-out from the
source's own track.

## Plan

The rest of the effect set landed as moq-scene `Filter`s that every
composition host renders with the same WGSL and the layout carries by name and
settings. A model cannot meet that parity rule: viewers would each need the
model and a GPU to run it, and inference differs across backends. So an ML
effect is baked into its source's track per the
[composition epic](/quest/m0/compose/README.md): it belongs to a source, never
to one scene item, and sits before the source's crop and filters, as the
encoder in `rs/moq-scene/src/publish.rs` sees the capture.

Prefer an existing runtime crate and a small, permissively licensed person
segmentation model (such as MediaPipe's selfie segmentation, Apache-2.0) over
training one. Running it on the studio's own wgpu device keeps the frame on
the GPU and works toward the [web studio](/quest/m0/web.md); a CPU runtime
costs a readback per frame. Measure the per-frame cost at 1080p and 30 fps
before choosing.
