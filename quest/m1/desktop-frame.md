# [L] See and crop what is shared, on the desktop itself

## Goal

A streamer working in another app sees on their real desktop which part of
each captured display or window is on air, without switching to the studio:
a frame outlines it, and whatever viewers cannot see of that capture is
dimmed (cropped away, covered by another layer, or past the canvas). In
**Build**, dragging the frame's edges trims the layer's crop live, as the
studio's Crop does, and dragging inside it moves the picture under it.

On macOS, Windows, and X11, for displays and windows.
[Wayland](/quest/m1/desktop-frame-wayland.md) and
[sharing from the desktop](/quest/m1/desktop-share.md) are their own quests.
Placing the layer on the canvas stays in the studio.

## Plan

Decided 2026-11-22 with the maintainer (/quest-plan). The problem: checking
what is public, and whether a crop is right, takes an alt-tab to the studio.

- **Shape:** ✅ a frame on the desktop, editable (rejected: only a floating
  program preview, which is [its own quest](/quest/m1/pop-out.md) since it
  also shows placement; shading without editing).
- **Two levels:** ✅ **View** draws the frames and the dimming and lets every
  click through, so it can stay on while working; **Build** adds the
  handles and the scope chip, and Escape leaves it. Build takes the pointer
  only over the handles, the frame's inside, and the chip; every other click
  still reaches the app below. Each has a hotkey, a
  palette entry, and a button in the toolbar of a selected display or window
  layer (rejected: Build only, which cannot stay on while working; tying
  Build to the title bar's Editable/Locked toggle, which would put handles
  over the desktop whenever the canvas is unlocked). Both hotkeys start
  unbound, as every hotkey does, and fire with a game in front.
- **Core, off by default:** ✅ not a mod (rejected: a mod; always shown while
  a capture is on air). Anyone sharing a screen asks what is public, and it
  adds one toolbar button, on layers that capture a screen.
- **What a drag does:** ✅ trims, as Crop does: the layer's box on the canvas
  grows or shrinks with the frame (rejected: a viewport that keeps the box
  and locks the frame's shape; refitting the box by its fit rule). The
  streamer fixes the layout in the studio afterwards. Growing snaps to, and
  stops at, the canvas's edges and every layer the box does not already
  overlap; going past them is done in the studio. A layer in a slot has no
  box of its own, so its frame keeps the slot's shape.
- **Which crop:** ✅ a chip on the frame picks **This scene**, the layer's own
  crop, which is the default, or **All scenes**, the source's crop, which
  changes every scene showing it (rejected: the layer's crop alone; a frame
  for every scene's use, which is busier than what is public). A drag is one
  undoable edit.
- **Which frames:** one per on-air layer showing a display or window, so a
  display shown twice, full and zoomed, has two. Dimmed means no output
  carries those pixels: the program in each format, a published scene, and
  the source published, recorded, or replayed on its own. When the source
  goes out whole that way, nothing is dimmed and the frame says why.
- **Never on stream:** on macOS and Windows the overlay windows are left out
  of capture (sharing type, display affinity), as
  [the privacy shield](/quest/m1/privacy-shield.md) does for the studio's
  windows. X11 cannot leave a window out, so there the overlay paints only
  pixels that are already off air. A handle on an edge with no off-air
  pixels beside it is an invisible grab strip: the input region needs no
  paint, and the pointer changes shape over it.
- **Covered:** recommended: the compositor reports, for each framed layer,
  how much of the capture still reaches each output pixel, mapped back into
  the capture's pixels, so a round camera dims exactly what it hides and a
  half-transparent lower third dims nothing. Opacity alone is wrong: an
  opaque layer in a blend mode such as Multiply leaves the capture visible.
  Where a blend mode or filter makes the contribution hard to know, count
  the pixel as on air, since dimming a live pixel is the error that leaks.
  Compute it only while the overlay is on, and measure its frame time.
- **Following a window:** a window's frame follows it as it moves, from
  moq-video's placement notices, and is hidden while the window is
  minimized or off screen.
- **Where it cannot run** (Wayland until [its quest](/quest/m1/desktop-frame-wayland.md)),
  the toolbar button says why rather than opening nothing; decide this at
  startup.

The PR adds the frame, in View and in Build, to
[`mock/later.html`](/quest/m0/ux/mock/later.html), shows the toolbar button
with `just shots`, and updates `README.md`.

Human check: on macOS, Windows, and X11 with two monitors at mixed scales,
that the frame lines up with a display and with a moving window, that a
second capture tool sees nothing of it on macOS and Windows, and how a drag
feels.

## Required

- [moq-video reports where each display sits](/quest/m1/pin-display-placement.md) - a display's rectangle on the desktop and its scale
- [moq-video's capture listing is pinned](/quest/m0/pin-capture-list.md) - a window's rectangle on its display, and notices when it moves

## Related

- [Pop out the program](/quest/m1/pop-out.md) - the whole program, where this shows one capture
- [The studio hides itself and private apps from capture](/quest/m1/privacy-shield.md) - the same capture exclusion, for the studio's own windows
