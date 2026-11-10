# [M] The studio hides itself and private apps from capture

## Goal

The studio's own windows never appear in a display capture, its own or
another app's. Apps the streamer marks private, such as a password manager
or a messenger, are left out of every display capture the studio makes. See
the `shield` frame of [`mock/later.html`](/quest/m0/ux/mock/later.html).

## Plan

- **The studio's own windows** ask the operating system to leave them out of
  capture, where it offers that: the window sharing type on macOS and the
  display affinity on Windows. This needs no moq change.
- **Private apps** are left out by moq-video's capture, which reaches here
  through [its pin](/quest/m1/pin-capture-exclude.md). moq plans that for
  macOS alone: no other desktop's capture can filter, and painting over a
  window instead would show it for a frame whenever it moves.
- The frame draws a lock where the private window was. A filtered capture
  shows what is behind the window instead, and a window's place on its
  display is not something moq reports, so there is no lock.
- The list of private apps is the operator's, kept per user, not in the scene
  file. Add marks an app private from its card.
- Where the desktop offers neither, as on X11 and for a portal-picked screen
  on Wayland, the setting says so and is off. It never claims a protection
  it cannot give: support is decided per desktop at startup, and a display
  source on an unprotected desktop carries a note in its inspector.

Settle here whether it also hides notification pop-ups, the commonest leak
in a whole-screen capture, where a desktop lets an app silence or exclude
them.

Human check: a second capture tool on each desktop, to see what it sees.

## Required

- [The BUILD page](/quest/m0/ux/build.md) - the inspector's note and the Add card's mark
- [moq-video's capture exclusion is pinned](/quest/m1/pin-capture-exclude.md) - leaving chosen apps out of a display capture
