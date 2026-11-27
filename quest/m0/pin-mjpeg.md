# [XS] moq's MJPEG camera fix is pinned

## Goal

The `moq` submodule includes moq-video's fix for MJPEG-only cameras, and a
Sony ZV-E10 (UVC, MJPEG 1280x720 only) shows a correct picture in the studio
instead of a squashed top half over a blue-tinted bottom half.

## Plan

Found 2026-11-25 trying the first launch on the maintainer's Fedora desktop,
camera added through PipeWire. Decoding is moq-video's
`I420::from_mjpeg`, so the fix lands in moq first. It landed 2026-11-26 as
[moq-dev/moq#9667](https://github.com/moq-dev/moq/pull/9667), after the API
breaks [pin-moq-tip](/quest/m0/pin-moq-tip.md) absorbs. Once that pin is on
`main`, add the camera on that desktop and check the picture.

## Hosts

- linux - the camera is on the maintainer's Fedora desktop

## Required

- [The studio catches up to moq's main](/quest/m0/pin-moq-tip.md) - the fix is past moq's API breaks that quest absorbs, so its `just upgrade-moq` carries it
