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
browser suite on the GPU path on NVIDIA (RTX 3070 Ti, GNOME on Wayland), and
on the CPU path with no display on NVIDIA and Intel.

Mesa did not paint in its only trial, on nixos2 (Intel UHD 630, Mesa 26.2.4,
beside an RTX 2070), under a headless sway on the Intel render node, since no
desktop session was logged in:

- Wayland: CEF's GPU process starts on Mesa's EGL, but the first shared image
  fails to allocate ("Unable to allocate frame for first frame capture:
  OOM?") and the page never paints. The same happens with glvnd pinned to
  Mesa (`__EGL_VENDOR_LIBRARY_FILENAMES`) and with `--disable-gpu-sandbox`.
  Release CEF strips the allocation's own log (a `DLOG` in
  `media/gfx/renderable_mappable_shared_image_video_frame_pool.cc`).
- X11, under that sway's XWayland: shared textures import, but come back
  empty (transparent pages, black WebGL).
- The same headless sway on the NVIDIA render node paints shared textures, so
  the harness itself works.

First check whether a real Mesa desktop session paints; the headless sway on
a hybrid host may be the cause. If it does not, find which allocation fails
(a debug CEF build, or `chrome://gpu` in a CEF client on the same host).

When the capturer cannot allocate, CEF never paints and never says so. Fail
the page loudly instead, from a signal CEF or Chromium gives (no first-paint
timeout).

## Waiting

- Maintainer: log into a Wayland desktop session (GNOME or KDE) on a Mesa GPU,
  such as nixos2's Intel, so an agent can run `binary(browser)` there with that
  session's `WAYLAND_DISPLAY`. An agent cannot start a logged-in session on a
  greeter. Check: `loginctl list-sessions` shows a user session on `seat0`.
