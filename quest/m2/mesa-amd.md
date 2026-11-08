# [XS] Browser sources paint on the GPU under Mesa AMD Wayland

## Goal

Browser sources copied on the GPU paint on an AMD GPU's Mesa driver under
GNOME on Wayland, as they do on Intel and NVIDIA.

## Plan

From [moq-dev/studio#111](https://github.com/moq-dev/studio/pull/111), which
fixed Intel: Mesa there picked a compressed, two-plane buffer layout
(`INTEL_Y_TILED_CCS`), and Chromium's frame capturer drops multi-plane
buffers, so the browser helper now starts with `INTEL_DEBUG=noccs`
(`rs/moq-scene/src/browser/run.rs`). AMD's compression (DCC) can add planes
the same way. Run `binary(browser)` on an AMD Mesa host, and only if it fails
the same way, add `AMD_DEBUG=nodcc` beside `noccs`.

Planned 2026-11-08 (maintainer, /quest-plan): ✅ m2, waiting on a host
(rejected: m0; dropping it), since no user has reported AMD failing.

## Waiting

- Maintainer: provide an AMD GPU host running Mesa with GNOME on Wayland,
  reachable over ssh. No known host has one, and an agent cannot acquire
  hardware. Check: `ssh <host> glxinfo -B` (or `vulkaninfo --summary`) names an
  AMD device on the radeonsi or RADV driver.
