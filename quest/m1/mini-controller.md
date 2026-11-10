# [M] A mini controller for one monitor

## Goal

A streamer with one monitor keeps a tiny bar over their game: the first few
scene pads, the microphone's mute, the live clock, the health dot, and the
panic key, always on top and never on stream. A tray or menu-bar item shows
whether the studio is live and offers the same actions. See the `mini` frame
of [`mock/later.html`](/quest/m0/ux/mock/later.html).

## Plan

- A second native window, borderless and always on top, drawing the same
  widgets as the LIVE page from the same state. Closing the main window to
  the tray leaves the bar and the stream running.
- It is hidden from capture by
  [the privacy shield](/quest/m1/privacy-shield.md), so a display capture
  never shows it.
- Where the desktop gives an app no way to stay on top, as on most Wayland
  compositors, the bar is not offered and Settings says why. Decide support
  per desktop at startup; never open a bar that will sink behind the game.
- It takes no keyboard focus from the game. Hotkeys remain the way to act
  without the mouse.

Human check: over a real full-screen game on each desktop, including
exclusive full screen on Windows, and that a third-party recorder does not
see the bar.

## Required

- [One live picture](/quest/m0/ux/live.md) - the widgets and state the bar repeats
- [The studio hides itself and private apps from capture](/quest/m1/privacy-shield.md) - the bar must not appear on stream
