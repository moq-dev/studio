# [S] Keypad keys bind as hotkeys

## Goal

The operator binds keypad keys (`Numpad0` to `Numpad9`, the keypad
operators, and keypad Enter) to studio actions, and they fire system-wide on
macOS, Windows, X11, and the Wayland portal, apart from the main-row keys
with the same symbol. Streamers often put scenes on the keypad.

## Plan

From [moq-dev/studio#61](https://github.com/moq-dev/studio/pull/61), which
refuses keypad keys rather than binding them; picked 2026-11-06 (maintainer,
/quest-plan) as its own quest so #61 lands as reviewed.

- Carry winit's physical `KeyCode` (or the key's keypad location) through
  `Binding`, so `Numpad1` and `Digit1` stay distinct.
- Register global-hotkey's `Numpad*` codes on macOS, Windows, and X11, and
  ask the GlobalShortcuts portal for `KP_*` keysyms on Wayland.
- `README.md` drops the note that keypad keys are refused.

## Required

- [Hotkeys and editor preferences](/quest/m1/hotkeys.md) - the bindings and settings store this extends
