# [XS] moq's MJPEG camera fix is pinned

## Goal

The `moq` submodule includes moq-video's fix for MJPEG-only cameras, and a
Sony ZV-E10 (UVC, MJPEG 1280x720 only) shows a correct picture in the studio
instead of a squashed top half over a blue-tinted bottom half.

## Plan

Found 2026-11-25 trying the first launch on the maintainer's Fedora desktop,
camera added through PipeWire. Decoding is moq-video's
`I420::from_mjpeg`, so the fix lands in moq first. Advance with
`just upgrade-moq` in its own PR, then add the camera on that desktop and
check the picture.

## Hosts

- linux - the camera is on the maintainer's Fedora desktop

## Waiting

- moq-dev/moq: [the MJPEG camera quest](https://github.com/moq-dev/moq/blob/main/quest/m1/mjpeg-camera.md) lands on moq's `main`. Check that quest's PR has merged.
