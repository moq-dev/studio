# [S] The Apple signing identity is available

## Goal

Release CI can sign and notarize the macOS app and its camera system
extension: moq-dev/studio holds, as Actions secrets for the release
environment, a Developer ID Application certificate with its key, an App
Store Connect API key for `notarytool`, and a Developer ID provisioning
profile carrying the System Extension capability.

## Plan

Decided 2026-11-06 (maintainer, /quest-unblock):

- ✅ **Windows ships unsigned for now.** Installers run, with a SmartScreen
  warning. Rejected: Azure Artifact Signing and an OV/EV certificate, both
  paid, for a build nobody downloads yet.
- The hang.live 1Password vault holds the Apple material for team
  `D7D5SDDB5Z`: `apple-developer-id` (a Developer ID Application certificate,
  valid until 2027-02-01), `apple-certificate` (`hang.live.p12` and its
  password), and `apple-api` (an App Store Connect API key, key ID
  `7WP7HJXJ4C`; its notes hold the issuer ID).
- Renewed 2026-11-07 (maintainer, /quest-unblock), and the Account Holder
  signed the Apple Developer Program License Agreement that day. The API key
  authenticates, but the notary API still answers
  `FORBIDDEN.REQUIRED_AGREEMENTS_MISSING_OR_EXPIRED` (see `/business`).
- Once the notary API accepts the key, an agent with the maintainer's `op`
  session creates the Developer ID profile through the App Store Connect API
  with that key, and stores everything with `gh secret set` (no secret passes
  through chat or a file outside `.tmp/`). Confirm the `.p12` holds the
  Developer ID key, not a distribution one, before storing it. Then delete
  this quest.

## Waiting

- Maintainer: accept any pending agreement at App Store Connect > Business
  (appstoreconnect.apple.com/business), which the notary API names after the
  Program License Agreement was signed; only the Account Holder can accept.
  If none is pending, Apple may still be propagating the signature. Check:
  `xcrun notarytool history` with the `apple-api` key succeeds (or a GET of
  `https://appstoreconnect.apple.com/notary/v2/submissions` with a JWT from
  that key returns 200).
