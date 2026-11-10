# [M] macOS app

## Goal

Every release ships a signed and notarized macOS app of `moq-studio`, built
in CI with the `release` feature and verified to launch and list devices on a
Mac.

This quest owns the app bundle: its layout, its `Info.plist` and
entitlements, and the step that signs and notarizes it. The
[camera extension](/quest/m0/vcam/macos.md) and
[browser sources](/quest/m0/browser-source/macos.md) add to that bundle
rather than build one of their own.

## Plan

The feature policy is the [epic](/quest/m0/packaging/README.md)'s.

Today the studio is not an app bundle, so macOS's capture prompts and grants
belong to whatever launched it. A bundle is asked itself, so it carries the
camera, microphone, and screen recording usage strings.

Lay the bundle out and order the signing so nested code can be added: a
system extension and CEF's helper apps are each signed before the app that
holds them.

## Required

- [The Apple signing identity is available](/quest/m0/signing.md) - the credentials macOS signing needs
- [Studio on macOS with capture granted](/quest/m0/app-macos.md) - camera, screen, and audio capture run on a Mac
