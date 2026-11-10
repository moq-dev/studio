# [L] Adding a source by picking a picture

## Goal

BUILD's Add tab shows what can be added as pictures, in the frames of
[`mock/add.html`](/quest/m0/ux/mock/add.html):

- **Apps:** every window as a live thumbnail with its app's real icon, its
  app's name, and its title, grouped by app.
- **Screens and cameras:** a live picture of each, with an icon for the kind
  of device (webcam, capture card, phone, display) and its name.
- **Sound:** each microphone and the system's sound with a moving meter, so
  the right one is found by talking.
- **Everything else:** files, a web page, text, guests, and other scenes,
  each a card.

A search field filters all of it, and a Recent row holds what was added
lately. A card is added by dragging it onto the picture, where it lands under
the pointer, or by selecting it and pressing one Add button. The list
refreshes itself when a device is plugged in or a window opens. No id is ever
typed.

It replaces the Devices panel and the add-source form, where a window is
chosen by typing its id.

## Plan

- The pictures and icons come from moq-video's capture listing, which today
  gives only ids, names, and sizes (`Window { id, title, app, width, height }`
  in `moq/rs/moq-video/src/capture/mod.rs`), with no icon, no thumbnail, no
  change notice, and apps listed on macOS alone. That work is planned in moq
  and reaches here through [its pin](/quest/m0/pin-capture-list.md).
- Where the operating system owns the picker, as on Wayland, the Apps and
  Screens sections are one button that opens the system's picker, and the
  choice is remembered: see the `system` frame and
  [Wayland sources](/quest/m0/wayland-sources.md).
- Recommended, not decided: a click selects and a button adds, rather than a
  click adding at once. A new layer is on air the moment it is added, and the
  selected card's picture is the last check against airing the wrong window.
- The inspector folds away while Add is open, since nothing is selected, so
  the picture keeps its width.
- The `apps` frame draws a "With its sound" switch. An app's own sound is not
  captured today, so the switch is left out until moq can.
- A card for something that cannot be added says why in its place (a kind
  this build lacks, a permission not granted) with the step that fixes it,
  rather than being absent.
- A dropped card lands where it is dropped, sized to its own shape, and is
  sent to the audio scene on program if it has sound, as an added source is
  today.
- The first-run screen reuses these cards.

Human check: real cameras, capture cards, and microphones, the operating
system's permission prompts, and whether a window is found faster than by
its title.

## Required

- [The BUILD page](/quest/m0/ux/build.md) - the column the Add tab lives in
- [moq-video's capture listing is pinned](/quest/m0/pin-capture-list.md) - icons, live pictures, and change notices for windows and devices

## Related

- [Wayland display and window sources](/quest/m0/wayland-sources.md) - the remembered system pick the Apps and Screens sections open
