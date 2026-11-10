# [M] Editing off air, and Studio mode

## Goal

Any scene can be opened for editing without going on air: the frame turns
amber, the tag reads OFF AIR, the live picture shrinks to a corner, and one
button takes the scene live or returns to the live one. See the
[off-air frame](/quest/m0/ux/mock/shell-offair.webp).

Studio mode, off by default, uses the same staging for a show run by an
operator. A switch at the end of the scene strip turns it on, and then a
scene click cues instead of taking: the pad turns amber, a NEXT monitor docks
on the picture, and TAKE or AUTO puts the cued scene on air, as in the
[Studio mode frame](/quest/m0/ux/mock/shell-studio.webp).

## Plan

Decided 2026-11-09 (maintainer, /quest-plan): ✅ cue and take (rejected: a
Preview and Program split, which halves the live picture and is what the
studio had; no mode at all, leaving only off-air editing).

- Both are the compositor's existing preview scene: off-air editing shows it
  large with the program small, and Studio mode shows it small with the
  program large. One mechanism, two arrangements.
- With Studio mode on, a scene's hotkey cues it, and the Cut and Transition
  actions are TAKE and AUTO. With it off, a scene's hotkey takes it. The
  Hotkeys settings say which, and the switch itself can be bound.
- Studio mode is kept per user with the other editor settings, not in the
  scene file: it is how one operator works, not part of the show.
- The NEXT monitor never covers the same corner as the item being dragged;
  it moves to the opposite one.
- Keyframes and overlays restart in the cued scene as they do today when a
  scene goes into preview, so NEXT shows the entrance the audience will see.

Human check: an operator's judgment that cue, TAKE, and AUTO feel like a
switcher, on a real show with several scenes.

## Required

- [One live picture](/quest/m0/ux/live.md) - the stage, the pads, and the tally frame this extends
