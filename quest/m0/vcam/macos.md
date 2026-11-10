# [M] macOS virtual camera

## Goal

The macOS app bundle carries a CoreMediaIO Camera Extension. Once the user
approves it, apps list a camera that carries the program or a published scene. An app
update installs the new extension without a reboot.

## Plan

Decided 2026-11-09 (maintainer, /quest-audit): ✅ the extension joins the
bundle the [macOS app](/quest/m0/packaging/macos.md) owns and goes through
its signing (rejected: a Related link only; leaving the quests unlinked).

## Required

- [macOS app](/quest/m0/packaging/macos.md) - the app bundle that carries and signs the extension
- [The Apple signing identity is available](/quest/m0/signing.md) - system extensions only load when signed
- [Studio on macOS with capture granted](/quest/m0/app-macos.md) - camera, screen, and audio capture run on a Mac
