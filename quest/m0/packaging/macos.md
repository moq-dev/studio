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

The signing material lives in the `release` Actions environment, which only
protected branches can deploy from, for team `D7D5SDDB5Z` and bundle ID
`live.kixel.studio`:

- `APPLE_CERTIFICATE`: the Developer ID Application `.p12`, base64, legacy
  encryption so `security import` reads it.
- `APPLE_CERTIFICATE_PASSWORD`: its password.
- `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_PRIVATE_KEY`: the App Store
  Connect key ID, issuer ID, and `.p8` text, for `notarytool --key`.
- `APPLE_PROVISIONING_PROFILE`: the Developer ID profile for `live.kixel.studio`
  with the System Extension capability, base64, embedded as
  `Contents/embedded.provisionprofile`.

The certificate expires 2027-02-01, and the profile with it; see
[renewal](/quest/m0/signing-renew.md).

## Waiting

- moq-dev/moq: a Mac camera renders only once AVFoundation asks for NV12 instead of `2vuy`, in review as [moq-dev/moq#8251](https://github.com/moq-dev/moq/pull/8251); done when it merges, `just upgrade-moq` bumps the gitlink here, and a Mac camera shows its picture in program
