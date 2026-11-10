# [M] Drop anything on the picture

## Goal

A file dragged from a file manager onto the picture becomes the right kind
of layer where it was dropped: an image, a video or sound file, a color
grade. A pasted image or web address does the same at the pointer. While the
drag is over the window, the picture shows where the layer will land and
what it will become. See the `drop` frame of
[`mock/later.html`](/quest/m0/ux/mock/later.html).

Nothing accepts a drop today.

## Plan

- A dropped or pasted file is copied beside the scene file, since sources
  name files relative to it, unless it is already under that directory.
- A file the studio cannot play says why where it was dropped, with the fix
  it has today: a regular MP4 names the remux that makes it readable.
- A pasted image has no file, so it is saved as a PNG beside the scene file.
- The windowing library reports dropped files only. Settle here whether a
  web address dragged from a browser is worth a hook per platform; paste
  covers it meanwhile.
- A drop on a layer that takes a file, such as an image layer, offers to
  replace its file instead.

## Required

- [The BUILD page](/quest/m0/ux/build.md) - the picture's drop targets and the layers it makes
