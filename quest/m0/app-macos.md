# [M] Studio on macOS

## Goal

`moq-studio` is run, not only compiled, on macOS, and whatever only shows up
there is fixed: camera, display, microphone, and system audio sources, plus
the window, preview, program, and publish path. Only Linux was built and run
when the studio landed.

## Plan

- Build with `--features capture`, since a default build has image, color,
  and broadcast sources only. That build fails on macOS today: the Devices
  panel's listing task holds moq-video's ScreenCaptureKit listing future,
  which is not `Send`, across a `runtime.spawn` in `rs/moq-studio/src/ui.rs`.
  Linux CI never builds it, so `just ci` cannot pass on a Mac until this is
  fixed. If the fix belongs in moq-video, land it there first.

## Related

- [Hardware validation](https://github.com/moq-dev/moq/blob/main/quest/m3/video-hardware.md) - capture paths not yet run on real machines
