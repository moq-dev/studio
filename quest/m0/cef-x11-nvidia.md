# [S] Browser sources on the GPU under X11 on NVIDIA

## Goal

On an NVIDIA GPU in an X11 or XWayland session, browser sources paint on
the GPU path. If the studio cannot give CEF a GBM device, it detects that at
startup and takes the CPU path with a note in Health, rather than leaving
pages stuck in Starting.

## Plan

From [moq-dev/studio#59](https://github.com/moq-dev/studio/pull/59), picked
2026-11-05 (maintainer, /quest-plan), m0 by the rule that fixes join m0.

- On nixos (RTX 3070 Ti, NVIDIA 580), the GPU path under X11 never paints:
  CEF's GPU process logs "Can't create buffer -- gbm device is missing".
  Wayland sessions avoid it since #59, but X11-only sessions still hit it.
- Since [moq-dev/studio#73](https://github.com/moq-dev/studio/pull/73) the X11
  path passes `--use-angle=gl-egl`, which made it paint on Mesa; rerun nixos
  with it first. On nixos2 under GNOME's XWayland, with the studio on the RTX
  2070, CEF still allocated on the Intel GPU that runs the X server (Intel
  X-tiled modifier) despite `--render-node-override`.
- First find why. Likely suspects: the NVIDIA GBM backend
  (`nvidia-drm_gbm.so`) is not on CEF's search path (`GBM_BACKENDS_PATH`), or
  `nvidia-drm` modeset is off. Fix it if the studio can, for example by
  passing the backend path to the helper or packaging it.
- Otherwise fail loud: probe for a GBM device when the helper starts, and if
  there is none, take the CPU path and say why in Health and the Sources panel.
  Fail on an observed probe result, never on a timeout.
- Verify on nixos under X11 (XWayland) and record how in the PR; no CI job
  has a GPU.
