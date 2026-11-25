# [L] Adding a source in one click

## Goal

Clicking `+ Add source` (or pressing `A`) opens a clean modal over the stage
([mock-up](/quest/m0/ux/mock/overhaul-add.webp), with live-card details in
[`mock/add.html`](/quest/m0/ux/mock/add.html)):

- **Devices & Screens:** cameras, displays, and app windows auto-populate the
  moment the modal opens (no manual `Refresh` button) with live thumbnails,
  real app icons, and hotplug updates. Where the OS owns the picker (Wayland),
  an App Window / Display card opens the system picker directly.
- **Media & Web:** clickable cards that open the native OS file picker for
  images and videos (no hand-typed file paths and no passive non-clickable
  hint rows), a single-field URL input for web pages, and a Color card.
- **Audio:** microphones and system audio with live level meters so the right
  input is obvious immediately.
- **Guests:** guest room card and invite link without being clipped off the
  bottom of a narrow sidebar.

Clicking any card adds it immediately with an auto-generated name and smart
non-overlapping placement (for example, a camera added over an existing
fullscreen screen lands cleanly as a corner PiP instead of covering the entire
canvas). Dragging a card onto the canvas places it at the drop point.

It deletes the legacy `Devices` refresh wall, the non-clickable `DROP OR PASTE
ANYTHING` rows, and the 17-kind `Any source` raw form where names and file
paths had to be typed by hand.

## Plan

Updated 2026-11-22 with the maintainer (/quest-plan):

- **Interaction model:** ✅ single click on any card adds the source
  immediately with an auto-generated name and smart placement, closing the
  modal with the new layer selected (rejected: two-click select then press an
  Add button; requiring a manual name before adding). Undo (`Ctrl+Z`) or
  `Delete` removes an accidental pick in one keypress.
- **Auto-refresh:** ✅ devices, screens, windows, and audio inputs enumerate
  automatically when the modal opens and refresh on hotplug (rejected: making
  the user click `Refresh` on an empty list).
- **Placement:** ✅ smart default placement: the first visual source fills/fits
  the canvas; a camera or overlay added on top of an existing full-canvas
  layer lands as a sensible corner PiP or centered aspect-fit box instead of
  hiding everything underneath (rejected: always `Rect::fill(&canvas)`).
- **Files & web:** ✅ Image and Media cards open `rfd::AsyncFileDialog`
  natively, and Web takes a single pasted URL (rejected: hand-typing paths or
  picking from a 17-item flat dropdown). Drag-and-drop and clipboard paste on
  the canvas continue to work directly.
- **Live thumbnails & icons:** come from moq-video's capture listing via
  [its pin](/quest/m0/pin-capture-list.md). Where the OS owns the picker
  (Wayland), the card invokes `picked_display` / `picked_window` directly and
  remembers the grant.
- **Errors in place:** a card that cannot be used (missing build feature or
  OS permission) explains why and offers the action to fix it instead of
  failing silently.

Found 2026-11-25 trying the first launch, kept here (✅ wait for this quest,
rejected: an interim Add tab fix now):

- The "Drop or paste anything" rows look like buttons and do nothing.
- The quick start's Camera, Screen, Media, and Web cards were the clearest way
  to add a source, and they vanish once the picture has one; the Add page
  offers the same cards.
- A camera plugged in after launch never appeared until **Refresh**.

Human check: real cameras, capture cards, and microphones, OS permission
prompts, and verifying 1-click placement feels right over a live screen.

## Required

- [moq-video's capture listing is pinned](/quest/m0/pin-capture-list.md) - icons, live pictures, and change notices for windows and devices

## Related

- The quick start on an empty picture (`rs/moq-studio/src/ui/start.rs`) adds a camera, a screen, a media file, or a web page or text in one click; the Add modal shares its device picks
