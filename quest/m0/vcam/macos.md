# [M] macOS virtual camera

## Goal

The macOS app bundle carries a CoreMediaIO Camera Extension. Once the user
approves it, apps list a camera that carries the program or a published scene. An app
update installs the new extension without a reboot.

## Required

- [Signing identities are available](/quest/m0/signing.md) - system extensions only load when signed
- [Studio on macOS with capture granted](/quest/m0/app-macos.md) - camera, screen, and audio capture run on a Mac
