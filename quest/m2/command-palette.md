# [M] A command palette

## Goal

One key opens a search box that runs anything the studio can do: take a
scene, mute a sound, add a window, start a recording, open a setting. It
lists each action with its key, and it is how a setting is found by name.

## Plan

Planned 2026-11-09 (maintainer, /quest-plan), and held for later: ✅ m2
(rejected: m1).

- Every action is registered once, with its name and the words it answers
  to, and that one table feeds the palette, the Hotkeys settings, and the
  tooltips, so they cannot disagree. Today the bindable actions are a fixed
  enum (`rs/moq-studio/src/controls.rs`).
- Scenes, sources, and settings are found by their own names.
- An action that cannot run now is shown with the reason, not hidden.

## Required

- [Settings](/quest/m0/ux/settings.md) - the settings it finds, and the Hotkeys pane that shares its table
