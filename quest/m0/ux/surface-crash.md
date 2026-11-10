# [S] The studio starts every time

## Goal

`moq-studio` no longer dies at startup with `wgpu error: Validation Error ...
In Surface::configure: Failed to wait for GPU to come idle before
reconfiguring the Surface`, followed by a panic. A test or a scripted run
launches it enough times, on a hardware GPU and on lavapipe, to show the crash
is gone.

## Plan

Found 2026-11-09 while taking screenshots for /quest-plan: 1 of about 11
launches under headless sway on radv, and 2 of 3 on lavapipe, each on the
launch that opens a scene file. Reproduce it first. The trigger is a guess to
confirm or discard: the compositor resizes the window right after it maps,
while the studio's own render thread keeps the shared device busy, so
eframe's reconfigure cannot get the device idle.

Fix the cause, not the symptom: no retry around `configure`. The compositor
and the UI share one device (`engine::Gpu`, handed to eframe as
`WgpuSetup::Existing` in `rs/moq-studio/src/main.rs`), so the fix likely
orders the engine's submissions against the surface's reconfigure.

No single PR caused it: the shared device is as old as the app. It is not
eframe's alone: the toolkit spike
([moq-dev/studio#126](https://github.com/moq-dev/studio/pull/126)) hit the
same error once in about 30 launches of its Slint page on `nixos`, whose
winit window configures its own surface on the shared device. So fix it
where the engine's submissions meet any window's reconfigure, and say in
the PR how the fix carries to the toolkit chosen there.

## Related

- [UI screenshots](/quest/m0/ux/shots.md) - cannot run routinely while a launch can crash
