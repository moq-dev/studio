# [XS] Try the multiview

## Goal

The maintainer tries the multiview on a real second monitor and either signs
it off or files what they find as quests: how it reads at a distance, which no
headless run can judge.

## Plan

Kept 2026-11-17 (maintainer, on
[moq-dev/studio#195](https://github.com/moq-dev/studio/pull/195#issuecomment-6941200815)).

The multiview is on `main` (`cargo run -p moq-studio --features capture`),
switched on as **Multiview** in Settings, Mods, then opened from the show's
menu, **Open the multiview**. What to try, on a show
with a few scenes and a camera or screen, going live to a relay:

- **At a distance:** from where you sit, whether the scene names, the ON AIR
  and NEXT tags, the accent and amber frames, the accent dot on a source in
  the live picture, and the clock read without leaning in.
- **Full screen:** F11 (or the square, or a double-click on the header) fills
  the monitor the window is on, not the main one, and Escape comes back.
- **Its place:** closing the studio with the multiview open, then opening it
  again, puts it back where it was, full screen if it was. Wayland lets the
  desktop pick where it opens, so there only the size and full screen come
  back.
- **Clicks:** a tile takes its scene, or cues it in Studio mode, and the
  scene's hotkey works with the multiview in front.
- **Live while hidden:** with the studio's window minimized or behind a game,
  the multiview stays live.
- **Sources:** whether the default picks (cameras, screens, windows, guests,
  web pages, video files, replays) are the ones you want, and **Leave out of
  the multiview** and the eye's list change them.
- **Look:** whether the window reads like its
  [mock-up](/quest/m0/ux/mock/later-multiview.webp).

An agent then turns each finding into a quest, or an edit to the quest that
owns that part of the window, and deletes this one.

## Waiting

- Maintainer: try the multiview as the Plan lists on a real second monitor and
  report what to change, or that nothing needs to. An agent cannot judge how
  it reads at a distance or place a window on a real monitor; a kittest run,
  `just shots`, and a headless sway run already cover the clicks, the layout,
  and the cost. Check: a comment or quest records the result.
