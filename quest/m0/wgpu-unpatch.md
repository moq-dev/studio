# [S] The studio builds on released wgpu

## Goal

The root `Cargo.toml` has no `[patch.crates-io]` for the wgpu crates: a wgpu
release carries the `Surface::configure` fix, and the studio's wgpu (and egui,
if the release is a new major) is bumped to it. The startup repro in
[moq-dev/studio#130](https://github.com/moq-dev/studio/pull/130) still shows
0 crashes in 30 or more lavapipe launches.

## Plan

[moq-dev/studio#130](https://github.com/moq-dev/studio/pull/130) patches
wgpu-core 30.0.1 from branch `surface-configure-race` of
[moq-dev/wgpu](https://github.com/moq-dev/wgpu/tree/surface-configure-race):
`Surface::configure` locks `Device::command_indices`, as `Queue::present`
does, then waits for the last submission and runs the hal configure under it.
Before, a submit from the compositor or encoder during configure's wait made
it fail with `GpuWaitTimeout`, and nothing kept a submit or present off the
hal configure's `vkDeviceWaitIdle`.

egui keeps rendering through wgpu on the shared device, so the upstream fix is
the real one. wgpu's suite has no native surface, so the fork's commit carries
no test; the studio's repro is the evidence.

Decided 2026-11-19 (maintainer, /quest-complete): ✅ wait on
[gfx-rs/wgpu#10675](https://github.com/gfx-rs/wgpu/pull/10675), another
author's PR that takes `command_indices` around the HAL surface configure and
unconfigure and the queue's drop, the race the fork fixes (rejected: opening
our own PR from `surface-configure-race`). It merged on
2026-11-20 UTC. If the release that carries it still crashes the startup repro,
open the fork's remaining change upstream then.

Once a release carries it, drop the whole `[patch.crates-io]` block. If it
lands only in a new wgpu major, bump egui and eframe to the release built on
it in the same PR.

## Waiting

- gfx-rs/wgpu: publish a release carrying
  [gfx-rs/wgpu#10675](https://github.com/gfx-rs/wgpu/pull/10675), merged on
  trunk 2026-11-20. Check: the wgpu-core changelog on crates.io lists it in a
  release.
