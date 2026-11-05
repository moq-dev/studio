# [S] Browser sources on the GPU without an X display

## Goal

Browser sources copied on the GPU (shared DMA-BUF textures) run under a pure
Wayland session without XWayland, and a studio with no display at all still
runs its browser sources, as the CPU path already does.

## Plan

From [moq-dev/studio#49](https://github.com/moq-dev/studio/pull/49), picked
2026-11-05 (maintainer, /quest-plan), m0 by the rule that fixes join m0.
[moq-dev/studio#55](https://github.com/moq-dev/studio/pull/55) moved the CPU
path to Chromium's headless Ozone platform and relayed Copy and Cut through
the studio's clipboard, so only the GPU path still opens an X display.

The GPU path cannot go headless: Chromium's headless Ozone platform allocates
`TestPixmap`s with no DMA-BUF
(`ui/ozone/platform/headless/headless_surface_factory.cc`), and in a trial its
GPU process failed to start EGL without a display, so pages fell back to
painting into memory (`Paint::Cpu`).

A trial on NixOS (RTX 3070 Ti, NVIDIA 580.178, GNOME on Wayland) on
2026-11-05 found `--ozone-platform=wayland`, with only `WAYLAND_DISPLAY` set,
painted shared textures (`Paint::Gpu`), and `a_page_draws_and_plays` passed.
On the same host the GPU path under XWayland (the default X11 Ozone platform,
`DISPLAY=:0`) never painted: "Can't create buffer -- gbm device is missing".

Recommended: pick the platform at setup in `Helper::new`, Wayland when
`WAYLAND_DISPLAY` is set, X11 when only `DISPLAY` is, and the CPU path when
neither is, which the Sources panel already shows as CPU. Check popups,
input, and the shared textures on Mesa (Intel or AMD) too, since the trial
covered only NVIDIA. No CI job has a GPU, so the PR records how it was
verified.
