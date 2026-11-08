# [S] Browser sources on the GPU under Mesa Wayland

## Goal

Browser sources copied on the GPU (shared DMA-BUF textures) paint on Mesa
drivers (Intel or AMD) under GNOME on Wayland, as they do on NVIDIA, or the
allocation that stops them is found and fixed.

## Plan

[moq-dev/studio#59](https://github.com/moq-dev/studio/pull/59) picks the
display at setup, X11 on Mesa paints since
[moq-dev/studio#73](https://github.com/moq-dev/studio/pull/73), and
[moq-dev/studio#107](https://github.com/moq-dev/studio/pull/107) passes over a
Wayland display with no output. Under a headless sway on nixos2's Intel render
node (UHD 630, Mesa 26.2.4), the first shared image fails to allocate
("Unable to allocate frame for first frame capture: OOM?") and the page stays
`Starting`; the same sway on the NVIDIA node paints, so the harness works.

Rerun `binary(browser)` on nixos2 in GNOME on Wayland with its display on the
Intel GPU, over ssh with `WAYLAND_DISPLAY=wayland-0`,
`XDG_RUNTIME_DIR=/run/user/1000`, and `VK_DRIVER_FILES` pointing at Mesa's
`intel_icd`. If it still fails to allocate, find which allocation fails (a
debug CEF build, or `chrome://gpu` in a CEF client on the same host) and fix
it. Surfacing that failure loudly was dropped in #107 (it needs CEF to forward
`OnStopped`).

Decided 2026-11-07 (maintainer, /quest-unblock): ✅ run this trial before
nixos2 switches to Windows ([windows-host](/quest/m0/windows-host.md)),
superseding [moq-dev/studio#105](https://github.com/moq-dev/studio/pull/105)'s
skip (rejected: keep the skip), with a display on the Intel GPU's own
motherboard port (rejected: the NVIDIA card's DisplayPort, a hybrid desktop).
It is on `card1-DP-1`, and GNOME on `wayland-0` lists a `wl_output` named
`DP-1`.
