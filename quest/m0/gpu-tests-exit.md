# [S] GPU tests exit cleanly on NVIDIA

## Goal

The compositor's GPU tests pass and exit cleanly on NVIDIA under Linux, as
they do on the software driver and on Metal.

## Plan

From [moq-dev/studio#74](https://github.com/moq-dev/studio/pull/74), picked
2026-11-07 (maintainer, /quest-complete), m0 by the rule that fixes join m0.

On nixos (RTX 3070 Ti, NVIDIA 580), 7 compositor GPU tests, mostly the
overlay ones, pass and then crash when the process exits. The same happens
with `main`'s Vulkan setup and without CEF, and not on Mesa's software
driver. Reproduce, find the cause (teardown order of the wgpu device and what
holds it is a first suspect), and fix it rather than skipping the tests.
