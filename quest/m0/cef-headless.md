# [S] Browser sources fail loudly when the GPU path cannot paint

## Goal

A browser source copied on the GPU (shared DMA-BUF textures) whose page cannot
paint fails its source saying why instead of staying `Starting`.

## Plan

When the first shared image fails to allocate, Chromium's frame sink video
capturer logs "Unable to allocate frame for first frame capture: OOM?" and
calls `Stop()`, which calls the consumer's `OnStopped()`. CEF's
`CefVideoConsumerOSR` drops `OnStopped` and `OnLog` (empty bodies in
`libcef/browser/osr/video_consumer_osr.h`), so no client callback fires and
the page stays `Starting`. Fail the page from that signal, not from a
first-paint timeout, once CEF forwards it.

Seen on nixos2 (Intel UHD 630 + RTX 2070): under a headless sway on the Intel
render node, and on 2026-11-07 under GNOME on Wayland with one connected
output (DP-1), every page hit that line and stayed blank. Whether shared
textures paint on Mesa under Wayland stays unknown; rerun `binary(browser)`
there once this lands.

A Wayland display with no `wl_output` is already passed over at setup, for
X11 or the CPU path.

## Waiting

- chromiumembedded/cef: forward the video consumer's `OnStopped` to the client,
  for example as a `CefRenderHandler` callback; check CEF's `video_consumer_osr.h`
  and the `cef` crate release that carries it. Advance it by opening an issue
  or PR upstream, which needs the maintainer's approval as it is outside
  `kixelated` and `moq-dev`.
