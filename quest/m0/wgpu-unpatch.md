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

Once a release carries it, drop the whole `[patch.crates-io]` block. If it
lands only in a new wgpu major, bump egui and eframe to the release built on
it in the same PR.

## Waiting

- Maintainer: open the gfx-rs/wgpu PR from `moq-dev/wgpu`'s
  `surface-configure-race` branch, rebased onto wgpu's trunk. wgpu's
  CONTRIBUTING requires the human submitter to write the PR description, so no
  agent posts there. Done when the PR is open.
- gfx-rs/wgpu: merge that PR and publish a release carrying it. Check the
  wgpu-core changelog on crates.io.
