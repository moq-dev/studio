# [M] Pop out the program

## Goal

A streamer with one monitor parks a small window showing the program alone
over or beside their work, so they see what is on air, and how it is
placed, without switching to the studio. It is borderless and resizable, takes
no keyboard focus, stays on top where the desktop lets an app ask, and is
never on stream where the desktop can leave it out of capture. It is a mod,
**Pop-out program**, off by default.

## Plan

Decided 2026-11-22 with the maintainer (/quest-plan).

- ✅ its own window (rejected: an expandable picture in
  [the mini controller](/quest/m1/mini-controller.md), which is not offered
  where the desktop cannot keep it on top, while a preview pinned by hand is
  still useful there).
- ✅ a mod (rejected: an entry for everyone in the show's menu).
- It draws the program texture the multiview already draws, and remembers
  its place and size per user, as the multiview does.
- On top: macOS, Windows, and X11 keep it there; on Wayland the streamer
  pins it from the desktop's own window menu, and the mod's help line says
  so.
- Hidden from capture by [the privacy shield](/quest/m1/privacy-shield.md)
  on macOS and Windows. Elsewhere a display capture can show it, so the
  mod's help line says to park it in what
  [the desktop frame](/quest/m1/desktop-frame.md) dims.

Human check: over a full-screen game on each desktop, and that a second
capture tool does not see it on macOS and Windows.

## Required

- [Mods](/quest/m0/ux/mods.md) - the pane that switches it on
- [The studio hides itself and private apps from capture](/quest/m1/privacy-shield.md) - the window must not appear on stream

## Related

- [A mini controller for one monitor](/quest/m1/mini-controller.md) - the always-on-top control bar
