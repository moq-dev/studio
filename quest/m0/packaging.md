# [M] Studio packaging

## Goal

Every release ships a signed and notarized macOS app, an unsigned Windows
installer, and a Linux Flatpak of `moq-studio`, all built in CI. Each artifact
is verified to launch and list devices on its platform.

## Plan

Decided 2026-10-26 (maintainer): as for the `moq` binary in
[Ship capture and playback](https://github.com/moq-dev/moq/blob/main/quest/m3/cli-packaging.md), moq-studio's defaults
skip any feature that is expensive to build or relies on a system library,
and a `release` feature turns on every backend its platform can load without
one (`capture`, `openh264`, `vaapi`, `nvidia`, and the platform frameworks).
Every artifact builds with `release`. `pipewire` stays out of `release`
because it links libpipewire at load time, but the Flatpak bundles
libpipewire, so it adds `pipewire` itself: inside the sandbox the camera
portal is the only way to a camera, and its PipeWire DMA-BUFs are what the
compositor imports without a copy.

Decided 2026-11-06 (maintainer, /quest-unblock): Windows ships unsigned
until a release needs SmartScreen reputation; see
[signing](/quest/m0/signing.md).

## Waiting

- moq-dev/moq: a Mac camera renders only once AVFoundation asks for NV12 instead of `2vuy`, in review as [moq-dev/moq#8251](https://github.com/moq-dev/moq/pull/8251); done when it merges, `just upgrade-moq` bumps the gitlink here, and a Mac camera shows its picture in program

## Required

- [The Apple signing identity is available](/quest/m0/signing.md) - the credentials macOS signing needs
- [Studio on Windows](/quest/m0/app-windows.md) - the studio running on Windows

## Related

- [Ship capture and playback](https://github.com/moq-dev/moq/blob/main/quest/m3/cli-packaging.md) - the same distribution checks for the `moq` binary
