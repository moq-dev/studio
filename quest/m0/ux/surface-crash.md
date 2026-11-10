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

No single PR caused it: the shared device is as old as the app. It does not
wait on the [toolkit spike](/quest/m0/ux/spike.md), since it is today's
studio that crashes, but say in the PR whether the fix would survive a move
off eframe.

## Related

- [UI screenshots](/quest/m0/ux/shots.md) - cannot run routinely while a launch can crash
