# [M] Studio on Windows

## Goal

`moq-studio` is run, not only compiled, on Windows, and whatever only shows
up there is fixed: camera, display, window, and microphone sources, plus the
window, preview, program, and publish path. Only Linux was built and run when
the studio landed.

## Plan

- System audio is `Unsupported` there until
  [Windows capture parity](https://github.com/moq-dev/moq/blob/main/quest/m3/capture-windows.md)
  adds WASAPI loopback, so that quest runs it.
- Build with `--features capture`, since a default build has no camera,
  display, window, microphone, or system-audio sources.
- Hotkeys, from [moq-dev/studio#84](https://github.com/moq-dev/studio/pull/84),
  only compiled for Windows so far: run the keypad keys, the refusal of
  keypad Enter (Windows' hotkey API cannot tell it from Enter), and a
  keyboard layout switch while a game has focus, which re-registers every
  binding on its physical key.
- The effects setting's read of Windows' animation setting
  (`rs/moq-studio/src/fx/motion.rs`, from
  [moq-dev/studio#85](https://github.com/moq-dev/studio/pull/85)) has not run
  there either.

## Required

- [nixos2 boots Windows for agents](/quest/m0/windows-host.md) - the Windows machine this runs on

## Related

- [Hardware validation](https://github.com/moq-dev/moq/blob/main/quest/m5/video-hardware.md) - capture paths not yet run on real machines
