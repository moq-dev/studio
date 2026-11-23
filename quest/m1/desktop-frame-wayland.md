# [M] The desktop frame on Wayland

## Goal

[The desktop frame](/quest/m1/desktop-frame.md), in View and Build, works on
Wayland desktops for a captured display: on KDE and wlroots desktops such as
Sway and Hyprland, and on GNOME if its workaround holds. A picked window
gets no frame, because Wayland never tells an app where another app's window
is, and its toolbar button says so.

## Plan

Decided 2026-11-22 with the maintainer (/quest-plan).

- **KDE and wlroots:** an overlay surface on wlr-layer-shell's overlay
  layer, with an input region that covers only the handles in Build and
  nothing in View. A picked monitor's place on the desktop is the position
  the ScreenCast portal reports for that stream, which
  [the display placement pin](/quest/m1/pin-display-placement.md) carries.
- **GNOME:** ✅ try an XWayland override-redirect window, which Mutter stacks
  above other windows, with an X11 input shape for click-through. Ship it on
  GNOME only if it holds on a real session at 100% and at a fractional
  scale, where X11 and Wayland coordinates can disagree, and over a game in
  full screen. Otherwise GNOME has no frame and the button says why
  (rejected: a GNOME Shell extension, which must follow every GNOME release
  and is installed apart from the app; leaving GNOME out untried).
- Neither desktop leaves a window out of capture, so the overlay paints only
  off-air pixels, as on X11.
- Which path a desktop gets is decided at startup, never by trying one and
  falling back.

Human check: GNOME at 100% and fractional scale, KDE, and Sway, each with
two monitors.

## Hosts

- linux - Wayland compositors

## Required

- [See and crop what is shared, on the desktop itself](/quest/m1/desktop-frame.md) - the frame this brings to Wayland
