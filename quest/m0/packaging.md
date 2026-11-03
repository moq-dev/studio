# [M] Studio packaging

## Goal

Every release ships a signed and notarized macOS app, a signed Windows
installer, and a Linux Flatpak of `moq-studio`, all built in CI. Each artifact
is verified to launch and list devices on its platform.

## Plan

Decided 2026-10-26 (maintainer): as for the `moq` binary in
[Ship capture and playback](https://github.com/moq-dev/moq/blob/main/quest/m2/cli-packaging.md), moq-studio's defaults
skip any feature that is expensive to build or relies on a system library,
and a `release` feature turns on every backend its platform can load without
one (`capture`, `openh264`, `vaapi`, `nvidia`, and the platform frameworks).
Every artifact builds with `release`. `pipewire` stays out of `release`
because it links libpipewire at load time, but the Flatpak bundles
libpipewire, so it adds `pipewire` itself: inside the sandbox the camera
portal is the only way to a camera, and its PipeWire DMA-BUFs are what the
compositor imports without a copy.

## Required

- [Signing identities are available](/quest/m0/signing.md) - the credentials signing needs
- [Studio on macOS with capture granted](/quest/m0/app-macos.md) - camera, screen, and audio capture run on a Mac
- [Studio on Windows](/quest/m0/app-windows.md) - the studio running on Windows

## Related

- [Ship capture and playback](https://github.com/moq-dev/moq/blob/main/quest/m2/cli-packaging.md) - the same distribution checks for the `moq` binary
