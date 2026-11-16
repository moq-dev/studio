# [XS] Try the BUILD page

## Goal

The maintainer tries the BUILD page on their own machines and either signs it
off or files what they find as quests: what the epic's
[What a person must check](/quest/m0/ux/README.md) asks of it, which no
headless run exercises.

## Plan

The BUILD page (its layers list, the editable picture, and the inspector) is
on `main` (`cargo run -p moq-studio --features capture`). What to try,
beside the [mock-up](/quest/m0/ux/mock/shell-build.webp):

- **Feel:** dragging, resizing, and cropping a layer on the picture with a
  real mouse and a trackpad; restacking layers by dragging their rows; how
  snapping and the grid hold a drag; Escape ending a crop, then the selection.
- **The inspector's order:** whether Place, Shape, Look, Shown in, and Source
  put first what a streamer reaches for first, and whether Animate belongs
  under the layer or beside it.
- **Real sources:** a camera, a capture card, and a display picked through the
  system picker, each added from Add, so the row's status dot and the
  inspector's Source section show a real device starting, failing, and coming
  back.
- **Interact:** typing into and clicking a web page layer under another page.

An agent then turns each finding into a quest, or an edit to the quest that
owns that page, and deletes this one.

## Waiting

- Maintainer: try the BUILD page as the Plan lists and report what to change,
  or that nothing needs to. An agent cannot judge feel or taste, and the agent
  hosts have no camera or capture card; a kittest run already covers the
  clicks, and `just shots` the layout. Check: a comment or quest records the
  result.
