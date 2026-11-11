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

The bundle's icon is an `.icns` that the build generates from
`packaging/live.kixel.studio.svg`, the only drawn copy, and `Info.plist`
names; never check the `.icns` in.

## Waiting

- moq-dev/moq: a Mac camera renders only once AVFoundation asks for NV12 instead of `2vuy`, in review as [moq-dev/moq#8251](https://github.com/moq-dev/moq/pull/8251); done when it merges, `just upgrade-moq` bumps the gitlink here, and a Mac camera shows its picture in program

## Required

- [The Apple signing identity is available](/quest/m0/signing.md) - the credentials macOS signing needs
