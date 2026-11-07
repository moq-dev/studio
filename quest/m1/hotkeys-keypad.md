# [M] Hotkeys bind by physical key

## Goal

A hotkey is the physical key the operator pressed, on every platform:
- Keypad keys (`Numpad0` to `Numpad9`, the keypad operators, and keypad
  Enter) bind apart from the main-row keys with the same symbol, and fire
  system-wide on macOS, Windows, X11, and the Wayland portal. Streamers often
  put scenes on the keypad.
- On X11, any key binds, including one that types a letter outside A to Z
  (such as ö on a German layout) and one winit cannot name.
- A keyboard layout change leaves every binding on its key without a restart:
  on X11 no held grab strands, and on Windows no binding moves to the key the
  new layout gives its virtual key.

## Plan

From [moq-dev/studio#61](https://github.com/moq-dev/studio/pull/61), which
refuses keypad keys and non-A-to-Z X11 letters, registers through the layout
in use when the studio starts or a key is bound, and asks for a restart after
an X11 layout change. Picked 2026-11-06 (maintainer, /quest-plan): keypad keys
as their own quest so #61 landed as reviewed, and the X11 keycode grab folded
in, since both rework the same grab path.

- Carry winit's `PhysicalKey` through `Binding`, taken from the window's
  winit key event rather than egui's, so `Numpad1` and `Digit1` stay distinct
  and a key winit leaves `Unidentified` keeps its native code. egui-winit
  drops a key it can name neither logically nor physically, so the Hotkeys
  panel cannot take it from egui. This replaces the keypad twin tracking in
  `rs/moq-studio/src/hotkeys/keypad.rs`.
- On X11, the studio grabs by keycode itself (XGrabKey through x11rb, which
  `layout.rs` already reads the keyboard mapping with) instead of through
  global-hotkey, which grabs by keysym and looks it up again in the current
  layout on release (see `Layout::releases` in `layout.rs`). The keycode is
  winit's Linux scancode plus 8 (`PhysicalKeyExtScancode::to_scancode`), for
  a named key and an `Unidentified` one alike, so releasing a grab needs no
  lookup and the restart note goes.
- On Windows, `RegisterHotKey` matches the virtual key the active layout gives
  the pressed key, so `layout.rs` maps each binding's scancode to a virtual key
  through that layout. The studio registers every binding again when that
  layout changes, not only when a binding does, as `hear` does now.
  Unregistering is by id, so nothing can strand. Recommended: read the
  foreground window's layout, which the system translates the key with, since
  a game usually has focus.
- Register global-hotkey's `Numpad*` codes on macOS and Windows, and ask the
  GlobalShortcuts portal for `KP_*` keysyms on Wayland. macOS registers by key
  position, so a layout change moves nothing there. A key winit leaves
  `Unidentified` is refused outside X11, saying global-hotkey cannot name it.
- `README.md`'s Hotkeys section drops the keypad refusal, the X11 letter
  refusal, the restart note, and the layout read when the studio starts or a
  key is bound.
