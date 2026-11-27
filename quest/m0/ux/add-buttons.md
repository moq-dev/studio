# [M] Add is a row of buttons

## Goal

Add opens as one row of plain buttons, with no tabs: Camera, Screen, Window,
File, Web page, Text, Color, Microphone, and Guest when guests are on. A
button that needs a choice opens one picker, then adds the pick and closes:

- Screen and Window use the system's picker where it is the only way to
  capture (a Wayland desktop), and the studio's own list everywhere else,
  never both side by side.
- Camera and Microphone open the studio's list of devices.
- File opens the system's file dialog.

Layers are named for what they show ("Screen", "Window", or the picked name),
never "Picked …". Adding a device the show already has reuses its source, and
deleting a layer also deletes its source once nothing else uses it, so the
"Not in any scene" section goes away.

## Plan

Found trying Add on 2026-11-27 (maintainer, [Try Add](/quest/m1/try-add.md)):
three "ZV-E10 (V4L2)" copies under "Not in any scene", and a "Picked display"
card beside an "HDMI-1" display that a Wayland desktop cannot capture.

Decided 2026-11-27 (maintainer, /quest-plan):

- ✅ Buttons, each opening one picker: the system's where it is the only way,
  the studio's own elsewhere (rejected: the system picker on every platform
  that has one, which loses the studio's own thumbnails; today's tabs).
- ✅ Fix the copies at the root: one source per device, and deleting the last
  layer that uses a source deletes it, which undo restores (rejected: only
  hiding "Not in any scene" and leaving copies in the show file).
- ✅ Drop the word "Picked".

Today: tabs in `rs/moq-studio/src/ui/add.rs`; "Not in any scene" is
`unused()` there, and its right-click "remove for good" goes with it. A device
card always makes a new, numbered source, and deleting a layer leaves its
source (`rs/moq-studio/src/ui/build.rs`). The picker card sits beside the X11
display list (`with_picker` in `rs/moq-studio/src/devices.rs`,
[moq-dev/studio#172](https://github.com/moq-dev/studio/pull/172)), whose
comment already says X11 cannot capture a Wayland desktop's screens. Decide
the platform's picker at setup from what the desktop offers, not by trying one
at runtime.

A Microphone button replaces the Audio tab, adding a layer (see
[Audio is layers](/quest/m0/ux/audio-layers.md)); until that lands, it adds
the sound as the Audio tab does today. Keep the placement rules Add already
has (a display first fills the picture, a camera over it goes to a corner).

Draw the new Add in [`mock/add.html`](/quest/m0/ux/mock/add.html) first, and
show its frame beside the `just shots` view in the PR.

## Related

- [Add shows pictures](/quest/m0/ux/add-pictures.md) - thumbnails in the studio's own pickers
- [A new layer takes its picture's own shape](/quest/m0/ux/source-size.md) - the size a picked layer lands at
