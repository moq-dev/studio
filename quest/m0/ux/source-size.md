# [S] A new layer takes its picture's own shape

## Goal

A layer added from Add, by a click or a drag, takes its source's own aspect
ratio instead of the canvas's: an image file, a video, a camera, and a
picked screen or window. Where the size is not known until the first frame,
the layer adopts it when that frame arrives, once, before anyone resizes it.
A picked window shows the window, with no black band around it.

## Plan

Found trying Add on 2026-11-27 (maintainer, [Try Add](/quest/m1/try-add.md)):
a screenshot added as a layer came out 16:9, and a picked window's layer
showed mostly black.

- A file dropped or picked through the file dialog already keeps its shape
  (`rs/moq-studio/src/ui/drop.rs`,
  [moq-dev/studio#193](https://github.com/moq-dev/studio/pull/193)). Add's
  cards build the picture with no size, so it falls back to the canvas shape
  and stretches (`shape` and `place` in `rs/moq-studio/src/ui/add.rs`,
  [moq-dev/studio#262](https://github.com/moq-dev/studio/pull/262)). Read the
  file's size as `drop.rs` does.
- Picked screens and windows have no size until frames arrive
  (`rs/moq-studio/src/devices.rs`). Only **Fit** and **Fill** read the live
  frame size (`rs/moq-studio/src/ui/build.rs`); reuse that once, on the first
  frame, for a layer nobody has resized. Record that it was adopted in the
  layer, so a reload does not adopt it again.
- The compositor stretches a picture over its box, so black inside the box
  is probably inside the frames. moq-video's PipeWire capture
  (`moq/rs/moq-video/src/capture/pipewire.rs`) requests no crop metadata,
  and some desktops send a window as a larger buffer with a crop region.
  Reproduce on the maintainer's GNOME desktop first. If the frames carry the
  black, the fix lands in moq-dev/moq with a moq-side test, then a pin quest
  here.

## Hosts

- linux - the picked-window black needs a real Wayland desktop and its portal
