# [S] One readback for the compositor's small GPU reads

## Goal

The compositor reads a few small pictures back from the GPU, a few times a
second each, through one shared helper: the dark-capture probe
(`rs/moq-scene/src/dark.rs`) and the keep-clear look
(`rs/moq-scene/src/place.rs`) each keep only their shader and what they
make of the tiles, and neither stalls a frame.

## Plan

The two landed separately with the same pattern: a compute pass reducing a
picture to a grid of tiles (64x36 for the probe, 32x18 for the look), at
most every 250 ms, its result copied to a buffer
mapped with `map_async` and read on a later frame, a failed or late read
dropped.

- **Shared:** the rate cap (`due`), the storage and readback buffers sized
  to the largest grid, the copy after the pass, the map in flight and its
  poll, and discarding a read whose source or feed changed meanwhile.
- **Kept apart:** each pipeline and its bindings. `dark.wgsl` reduces the
  capture texture alone to mean color and brightest channel; `place.wgsl`
  compares the items drawn under a layer with the last read, so it keeps
  two pictures that ping-pong.
- Their tests (`dark::tests::`, `compositor::tests::a_camera_keeping_clear_glides_off_what_moves`,
  `a_moment_of_dark_draws_the_last_good_picture`) pass unchanged under
  `just render-sw`, and the frame time on the AMD iGPU does not grow.

Decided 2026-11-17 (maintainer, on
[#194](https://github.com/moq-dev/studio/pull/194#issuecomment-6941200435)):
✅ one helper once both land (rejected: sharing in #194, before #192 lands).
