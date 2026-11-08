# [S] Browser sources fail loudly when the GPU path cannot paint

## Goal

A browser source copied on the GPU (shared DMA-BUF textures) whose page cannot
paint fails its source saying why instead of staying `Starting`, and a Wayland
display with no output is ruled out at setup rather than hanging CEF's
`initialize`.

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

Decided 2026-11-07 (maintainer, /quest-unblock): ✅ skip the Wayland trial on
Mesa (rejected: an HDMI dummy plug on nixos2's Intel GPU), so nixos2 can switch
to Windows ([windows-host](/quest/m0/windows-host.md)). Whether shared
textures paint on Mesa under Wayland stays unknown; the loud failure above is
what a user on such a setup sees until someone with that hardware reruns
`binary(browser)` there.
