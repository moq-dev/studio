# [S] Studio on macOS with capture granted

## Goal

On the maintainer's Mac, an agent runs `moq-studio --features capture` and
fixes whatever only shows up there: camera, display, window, microphone, and
system audio sources reaching preview, program, and the published broadcast,
and the Devices panel listing and adding each.

The build, `just ci`, the window, compositing on Metal, and publishing through
VideoToolbox already run on an Apple Silicon Mac. So does the microphone: its
sound reaches the published broadcast (2026-11-06, MacBook Air).

## Plan

- The studio is not an app bundle, so the prompts and grants belong to the app
  that launched it: T3 Code holds Camera, Microphone, and Screen & System Audio
  Recording. A run that still meets a permission error stops and asks the
  maintainer rather than working around it.
- Check a run from the broadcast, not only the window: record it from a local
  relay with `moq export mp4` and pull frames and loudness with ffmpeg.
- The built-in camera fails in the studio with `cannot download pixel format
  0x32767579; expected NV12`: AVFoundation capture never asks for NV12, so the
  camera delivers its native `2vuy`, which the Metal import refuses and the CPU
  download cannot read. Asking the output for NV12 (`videoSettings` with
  `kCVPixelBufferPixelFormatTypeKey`) brings the camera to program and the
  broadcast. That fix belongs in moq-video.
- With that fix, a camera source's track logs groups of about 3 s against its
  declared 2 s join interval. AVFoundation reports no frame rate, so the track
  assumes 30 fps and keyframes every 60 frames, which a dim room's 20 fps
  stretches. Likely a moq-video follow-up as well (report the device's rate, or
  keyframe by time).

## Waiting

- Maintainer: quit and reopen T3 Code. Its Screen & System Audio Recording grant (2026-11-06) postdates the running instance (started 2026-11-04), so the system still refuses it: display sources fail with `capture permission denied: screen recording access`, system audio with `The user declined TCCs for application, window, display capture`, and `screencapture` from an agent shell prints `could not create image from display`. Check with `screencapture -x .tmp/check.png` from an agent's shell.
- Maintainer: grant T3 Code Accessibility, so an agent can click the Devices panel's Refresh and add buttons; without it the window can be watched but not driven (`CGPreflightPostEventAccess` is false). The alternative is clicking through the panel by hand while an agent watches the log. Recommended: grant it.
- moq-dev/moq: AVFoundation camera capture asks its output for NV12, so a camera whose native format is `2vuy` reaches the renderer; then bump the `moq` gitlink. Check that `rs/moq-video/src/capture/avfoundation.rs` sets `videoSettings` on moq's `main`.
