# [M] Share a window from the desktop

## Goal

In the [desktop frame](/quest/m1/desktop-frame.md)'s Build, every window and
display on the desktop shows a **Share** button over it. Share adds it to the
on-air scene as a new layer, placed as the Add page places a new source, and
its frame appears at once. While a display or window layer is selected, a
**Swap** button beside it puts that window or display in the selected layer
instead, keeping the layer's place and crop. On macOS, Windows, and X11; on
Wayland the system's picker stays the way to pick one, and Build shows no
Share buttons.

## Plan

Decided 2026-11-22 with the maintainer (/quest-plan): ✅ both Share and Swap
(rejected: Share alone; Swap alone). Swap is the everyday move when the
streamer switches apps, and Share is how a new one starts.

- The windows and their rectangles come from moq-video's capture listing,
  already pinned for the frame, and the buttons follow its change notices.
- The studio's own windows and [private apps](/quest/m1/privacy-shield.md)
  get no button.
- Where the overlay cannot be left out of capture, as on X11, a button over
  pixels already on air would show on stream: place it in off-air pixels, or
  leave that window without one.

Human check: on macOS, Windows, and X11, sharing and swapping overlapping
windows on two monitors.

## Required

- [See and crop what is shared, on the desktop itself](/quest/m1/desktop-frame.md) - Build, and the overlay the buttons are drawn on
- [Adding a source in one click](/quest/m0/ux/add.md) - where a new layer is placed
