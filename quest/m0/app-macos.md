# [S] Studio on macOS with capture granted

## Goal

On the maintainer's Mac, an agent runs `moq-studio --features capture` and
fixes whatever only shows up there: camera, display, window, microphone, and
system audio sources reaching preview, program, and the published broadcast,
and the Devices panel listing and adding each.

The build, `just ci`, the window, compositing on Metal, and publishing through
VideoToolbox already run on an Apple Silicon Mac. Capture has not.

## Plan

- The studio is not an app bundle, so the prompts and grants belong to the app
  that launched it. Granted 2026-11-06 (maintainer, /quest-unblock): T3 Code
  holds Camera, Microphone, and Screen & System Audio Recording, so an agent
  it launches captures without a prompt. A run that still meets a permission
  error stops and asks the maintainer rather than working around it.

## Related

- [Hardware validation](https://github.com/moq-dev/moq/blob/main/quest/m5/video-hardware.md) - capture paths not yet run on real machines
