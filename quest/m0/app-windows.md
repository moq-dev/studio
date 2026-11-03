# [M] Studio on Windows

## Goal

`moq-studio` is run, not only compiled, on Windows, and whatever only shows
up there is fixed: camera, display, window, and microphone sources, plus the
window, preview, program, and publish path. Only Linux was built and run when
the studio landed.

## Plan

- System audio is `Unsupported` there until
  [Windows capture parity](https://github.com/moq-dev/moq/blob/main/quest/m2/capture-windows.md)
  adds WASAPI loopback, so that quest runs it.
- Build with `--features capture`, since a default build has image, color,
  and broadcast sources only.

## Related

- [Hardware validation](https://github.com/moq-dev/moq/blob/main/quest/m3/video-hardware.md) - capture paths not yet run on real machines
