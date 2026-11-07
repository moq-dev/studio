# [S] Browser sources on the GPU under Mesa

## Goal

Browser sources copied on the GPU (shared DMA-BUF textures) paint on Mesa
drivers (Intel or AMD) under Wayland, as they do on NVIDIA, and a page whose
GPU path cannot paint fails its source saying why instead of staying
`Starting`.

## Plan

[moq-dev/studio#59](https://github.com/moq-dev/studio/pull/59) picks the
display at setup: Chromium's Wayland Ozone platform when `$WAYLAND_DISPLAY` is
set, X11 when only `$DISPLAY` is, and the CPU path with neither. It passes the
browser suite on the GPU path on NVIDIA (RTX 3070 Ti, GNOME on Wayland).

Found on nixos2 (Intel UHD 630, Mesa 26.2.4, beside an RTX 2070):

- X11 on Mesa paints since
  [moq-dev/studio#73](https://github.com/moq-dev/studio/pull/73). ANGLE drew
  through GLX there, which left every shared texture blank; the X11 path now
  passes `--use-angle=gl-egl`.
- Wayland is still unknown on Mesa. Under a headless sway on the Intel render
  node, the first shared image fails to allocate ("Unable to allocate frame
  for first frame capture: OOM?") and the page never paints. That headless
  sway on the NVIDIA node paints, so the harness works.
- GNOME with no monitor attached cannot run the Wayland path on any GPU. It
  advertises no `wl_output`, and Chromium's `WaylandConnection::Initialize`
  round-trips until one appears, so CEF's `initialize` never returns. The
  helper hangs before reading a command and every page stays `Starting`.
  Meanwhile `gnome-shell` uses a full core.

Remaining:

- Rerun `binary(browser)` on Wayland on the Intel GPU in GNOME with a display
  connected, over ssh with `WAYLAND_DISPLAY=wayland-0`,
  `XDG_RUNTIME_DIR=/run/user/1000`, and `VK_DRIVER_FILES` pointing at Mesa's
  `intel_icd`. If it still fails to allocate, find which allocation fails (a
  debug CEF build, or `chrome://gpu` in a CEF client on the same host).
- Fail a page loudly when the GPU path cannot paint, from a signal CEF or
  Chromium gives (no first-paint timeout):
  - On allocation failure, Chromium's frame sink video capturer logs the OOM
    line and calls `Stop()`, which calls the consumer's `OnStopped()`. CEF's
    `CefVideoConsumerOSR` drops `OnStopped` and `OnLog` (empty bodies in
    `libcef/browser/osr/video_consumer_osr.h`), so no client callback fires.
    Surfacing it likely needs a CEF change upstream.
  - When `$WAYLAND_DISPLAY` has no `wl_output`, decide that at setup rather
    than hang in `initialize`: for example, check for an output in `raster()`
    and fall to X11 or the CPU path.

Decided 2026-11-06 (maintainer, /quest-unblock): this trial runs before
nixos2 switches to Windows ([windows-host](/quest/m0/windows-host.md)), which
then leaves no Mesa machine on NixOS.

## Waiting

- Maintainer: connect an HDMI dummy plug (or a monitor) to nixos2's
  motherboard HDMI port, which the Intel GPU drives, since GNOME offers
  Chromium no Wayland output without one. An agent cannot plug hardware, and
  a virtual monitor would test a different setup from a real desktop. Check
  that `cat /sys/class/drm/card1-HDMI-A-*/status` shows `connected` and that
  `wayland-info` on `wayland-0` lists a `wl_output`. Alternative: skip the
  Wayland trial on Mesa and let nixos2 switch to Windows, leaving the Wayland
  result unknown. Recommended: the dummy plug.
