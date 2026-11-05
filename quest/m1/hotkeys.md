# [L] Hotkeys and editor preferences

## Goal

The operator binds keys to studio actions, and they fire system-wide, even
while another app such as a game has focus:
- replay
- save a clip
- cut, transition, and select a scene
- start and stop recording and going live

In the focused preview, arrow keys nudge the selected item, and holding a
modifier while resizing locks its aspect ratio.
Snap and grid settings and the hotkey bindings persist in an app settings
store, apart from the scene file.

## Plan

From [moq-dev/studio#15](https://github.com/moq-dev/studio/pull/15) and
[moq-dev/studio#20](https://github.com/moq-dev/studio/pull/20).

Decided 2026-11-04 with the maintainer (/quest-plan):

- ✅ **Global hotkeys in this quest**, as OBS has, since an operator fires a
  replay from a game. Rejected: focused-only first with global later.
- ✅ **No default bindings.** Every action starts unbound, like OBS, so a
  global key never collides with a game. Arrow-key nudge is always on but
  works only in the focused preview.

Implementation:

- Per platform: the XDG GlobalShortcuts portal on Wayland (falling back to
  focused-only where the portal is missing, said in the UI), `RegisterHotKey`
  on Windows, and an event tap on macOS, which needs the Input Monitoring
  grant; ask for it when the first key is bound. Prefer a maintained crate
  that covers these over writing each.
- The settings store lives in the platform config directory, not the scene
  file, so a scene file moves between machines without carrying one
  operator's keys.
- A Hotkeys panel binds and clears keys, and shows a conflict.
- Aspect lock: the default for video sources, which have a fixed shape.
- `README.md` documents the panel, the macOS grant, and the Wayland portal.
