# [M] Hotkeys bind by physical key

## Goal

A hotkey is the physical key the operator pressed, on every platform:
- Keypad keys (`Numpad0` to `Numpad9`, the keypad operators, and keypad
  Enter) bind apart from the main-row keys with the same symbol, and fire
  system-wide on macOS, Windows, X11, and the Wayland portal. Streamers often
  put scenes on the keypad.
- On X11, any key binds, including one that types a letter outside A to Z
  (such as ö on a German layout), and a keyboard layout change neither strands
  a held grab nor needs a restart.

## Plan

From [moq-dev/studio#61](https://github.com/moq-dev/studio/pull/61), which
refuses keypad keys and non-A-to-Z X11 letters, and asks for a restart after
an X11 layout change. Picked 2026-11-06 (maintainer, /quest-plan): keypad keys
as their own quest so #61 landed as reviewed, and the X11 keycode grab folded
in, since both rework the same grab path.

- Carry winit's physical `KeyCode` through `Binding`, so `Numpad1` and
  `Digit1` stay distinct. This replaces the keypad twin tracking in
  `rs/moq-studio/src/hotkeys/keypad.rs`.
- On X11, the studio grabs by keycode itself (XGrabKey through x11rb, which
  `layout.rs` already reads the keyboard mapping with) instead of through
  global-hotkey, which grabs by keysym and looks it up again in the current
  layout on release (see `Layout::releases` in `layout.rs`). Releasing a
  grab then needs no lookup, so the restart note goes.
- Register global-hotkey's `Numpad*` codes on macOS and Windows, and ask the
  GlobalShortcuts portal for `KP_*` keysyms on Wayland.
- `README.md`'s Hotkeys section drops the keypad refusal, the X11 letter
  refusal, and the restart note.
