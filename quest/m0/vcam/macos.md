# [M] macOS virtual camera

## Goal

The macOS app bundle carries a CoreMediaIO Camera Extension. Once the user
approves it, apps list a camera that carries the program or a published scene. An app
update installs the new extension without a reboot.

## Required

- [The Apple signing identity is available](/quest/m0/signing.md) - system extensions only load when signed
