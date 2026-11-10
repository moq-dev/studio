# [S] The studio starts every time

## Goal

`moq-studio` no longer dies at startup with `wgpu error: Validation Error ...
In Surface::configure: Failed to wait for GPU to come idle before
reconfiguring the Surface`, followed by a panic. A test or a scripted run
launches it enough times, on a hardware GPU and on lavapipe, to show the crash
is gone.

## Plan

Reproduced 2026-11-10 under a private headless sway: with the output resized
200 times in the first seconds, 8 of 10 launches crash on lavapipe and 0 of
10 on an NVIDIA GPU; without resizes, 0 of 10. The claim PR holds the script.

The cause is in wgpu-core 30.0.1, not the studio. `Surface::configure` waits
for the queue to drain, then fails with `GpuWaitTimeout` if it is not empty,
which only happens when another thread submitted during the wait: here the
compositor and encoder on the device the UI shares (`engine::Gpu`). A local
wgpu-core patch accepting that case took lavapipe to 0 of 30, confirming the
cause, but is not the fix: configure's `vkDeviceWaitIdle` also needs every
queue externally synchronized, so no submit may land during it either. The
fix belongs in wgpu: hold `Device::command_indices` across the idle wait and
the hal configure, as `Queue::present` already does.
[gfx-rs/wgpu#10799](https://github.com/gfx-rs/wgpu/pull/10799) took that lock
around the hal configure but was closed unmerged.

A studio-side lock would have to wrap every submit on the shared device, in
moq-scene, moq-video's export and readback, and every future caller, so it is
the fallback only if wgpu declines.

The fix survives a move off eframe: any toolkit that shares the device and
configures a wgpu surface goes through the same `Surface::configure`.

## Waiting

- Maintainer: approve posting the fix to gfx-rs/wgpu, outside the orgs an
  agent may post to. Options: (a) fix wgpu upstream, then bump wgpu and egui
  here; (b) a studio-wide submit lock, after a moq-video change. Recommend
  (a); check by the approval comment on the claim PR.

## Related

- [UI screenshots](/quest/m0/ux/shots.md) - cannot run routinely while a launch can crash
