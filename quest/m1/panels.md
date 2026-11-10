# [L] Private panels

## Goal

Any web page can be kept beside the show as a private panel: a chat, an
activity feed, a seller's dashboard. A panel is its own window, which can sit
on another monitor or stay on top, and it can be docked into spare room
beside the picture. It is never on stream: a panel cannot be placed in a
scene, and its window is hidden from capture. See the `panels` frame of
[`mock/later.html`](/quest/m0/ux/mock/later.html).

## Plan

Decided 2026-11-09 (maintainer, /quest-plan): ✅ its own window, dockable,
nothing docked by default (rejected: a chat column in the main window, which
puts chat beside the streamer's own face all show; leaving chat to the
browser). This is how the studio meets platforms without naming one: the
page is the platform's own.

- A panel is rendered by the browser the studio already ships for browser
  sources, with the same profile, so a login made once holds for both. It
  takes the mouse and keyboard as **Interact** does.
- Panels are the operator's, kept per user with a name and an address, not
  in the scene file.
- Docking uses the room the layout has left after the picture, the scenes,
  and the sound, and is not offered when there is none.
- The panel's window is hidden from capture by
  [the privacy shield](/quest/m1/privacy-shield.md).
- A page can do anything the browser can, as a browser source can: the same
  warning to add only pages you trust applies, and is shown when adding one.

Browser sources exist on Linux only today, so panels arrive on macOS and
Windows with them.

Human check: a real chat page, logged in, typed into during a stream.

## Required

- [One live picture](/quest/m0/ux/live.md) - the layout a panel docks into
- [The studio hides itself and private apps from capture](/quest/m1/privacy-shield.md) - a panel must not appear on stream

## Related

- [Browser sources on macOS and Windows](/quest/m0/browser-source/README.md) - the browser panels render with on the other desktops
