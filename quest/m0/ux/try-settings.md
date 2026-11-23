# [XS] Try the Settings window

## Goal

The maintainer tries the Settings window on their own machine and either signs
it off or files what they find as quests: what the epic's
[What a person must check](/quest/m0/ux/README.md) asks of it, which no
headless run exercises.

## Plan

The Settings window is on `main` (`cargo run -p moq-studio --features capture`),
opened from the title bar's gear. What to try, on a show going live to a relay:

- **Placement:** whether each setting is where a streamer would look for it,
  across Show, Stream, Recording, Hotkeys, Guests, and Mods, and Remote once
  Phone remote is switched on in Mods.
- **Words:** each pane's lead line, each setting's help line, and the cost a
  line names (the canvas size's upload, an encoder per published scene).
- **On air:** a new canvas size or frame rate while live asks "Restart the
  video?" first; whether that reads as a warning and not a block.
- **Guests:** saving a room joins it at once off air, and a bad room or key is
  refused with the reason.
- **Look:** whether the window reads like its
  [mock-up](/quest/m0/ux/mock/settings-show.webp).

An agent then turns each finding into a quest, or an edit to the quest that
owns that part of the window, and deletes this one.

## Waiting

- Maintainer: try the Settings window as the Plan lists and report what to
  change, or that nothing needs to. An agent cannot judge placement or words;
  a kittest run and `just shots` already cover the clicks and the layout.
  Check: a comment or quest records the result.
