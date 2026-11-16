# [M] macOS virtual camera

## Goal

The macOS app bundle carries a CoreMediaIO Camera Extension. Once the user
approves it, apps list a camera that carries the program or a published scene. An app
update installs the new extension without a reboot.

## Plan

Decided 2026-11-09 (maintainer, /quest-audit): ✅ the extension joins the
studio's app bundle and goes through its signing (rejected: a Related link
only; leaving the quests unlinked).

`packaging/macos/bundle.sh` lays the bundle out; the extension goes in
`Contents/Library/SystemExtensions`. `packaging/macos/sign.sh` signs it, with
its own entitlements, before the app. The app then needs the
`com.apple.developer.system-extension.install` entitlement, which only a
provisioning profile grants: embed the `release` environment's
`APPLE_PROVISIONING_PROFILE` (the Developer ID profile for
`live.kixel.studio` with the System Extension capability, base64) as
`Contents/embedded.provisionprofile`, and sign with the
`com.apple.application-identifier` and `com.apple.developer.team-identifier`
entitlements it names. A pull request's ad hoc signature cannot carry these,
so its launch check needs them left out.
