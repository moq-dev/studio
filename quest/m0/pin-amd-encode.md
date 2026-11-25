# [XS] moq's AMD Vulkan encoder is pinned

## Goal

The `moq` submodule includes moq-video's Vulkan Video encoder for AMD, and a
new show on the maintainer's Radeon starts at 1080p60 on it, with no CPU
round trip.

## Plan

Found 2026-11-25 trying the first launch: VA-API cannot read the canvas on
the GPU, so each frame goes through a CPU readback. A release build timed it
at 14 ms a frame against the 12.5 ms 60 fps budget (`PACE` in
`rs/moq-studio/src/probe.rs`), so the show started at 30 fps.

Decided 2026-11-25 with the maintainer (/quest-plan): ✅ pull moq's AMD
Vulkan encode forward, from its m5 to m3 with the studio as its consumer
(rejected: speeding up the readback route, which stays a CPU round trip;
accepting 30 fps on AMD until then). This machine's RADV (Nix Mesa 26.3,
RDNA2 iGPU) exposes `VK_KHR_video_encode_h264` and `_h265`.

Advance with `just upgrade-moq` in its own PR. Validate on that desktop with
a release build: a new show's canvas runs at 60 fps, and the probe picked the
Vulkan encoder reading the canvas on the GPU, not the readback route.

## Hosts

- linux - the Radeon is on the maintainer's Fedora desktop

## Waiting

- moq-dev/moq: [Vulkan Video encode on AMD](https://github.com/moq-dev/moq/blob/main/quest/m3/vulkan-encode.md) lands on moq's `main`. Check that quest's PR has merged.

## Required

- [The dev shell runs the studio on a non-NixOS GPU](/quest/m0/dev-shell-gpu.md) - the Fedora desktop needs Nix Mesa's Vulkan driver to reach the encoder
