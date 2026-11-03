# [S] Studio on macOS with capture granted

## Goal

Someone at a Mac grants the camera, microphone, and Screen & System Audio
Recording permissions, runs `moq-studio --features capture`, and fixes
whatever only shows up there: camera, display, window, microphone, and system
audio sources reaching preview, program, and the published broadcast, and the
Devices panel listing and adding each.

The build, `just ci`, the window, compositing on Metal, and publishing through
VideoToolbox already run on an Apple Silicon Mac. Capture has not: its TCC
prompts need a person to answer them, so an unattended agent cannot do this.

## Plan

- The studio is not an app bundle, so the prompts and grants belong to the app
  that launched it, such as the terminal.

## Related

- [Hardware validation](https://github.com/moq-dev/moq/blob/main/quest/m3/video-hardware.md) - capture paths not yet run on real machines
