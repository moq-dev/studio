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
- Renewed 2026-11-08, until 2027-11-08 (maintainer, /quest-unblock), after the
  Account Holder signed the Apple Developer Program License Agreement and
  accepted the agreements pending at App Store Connect > Business. On
  2026-11-08 `xcrun notarytool history` with the `apple-api` key succeeds.

Decided 2026-11-08 (maintainer):

- ✅ Bundle ID `live.kixel.studio`, on the kixel.live domain (rejected:
  `dev.moq.studio`; `studio.moq.app`; kixel.studio, held by another party).
  The camera extension's ID derives from it.
- ✅ Keep the secret names listed in [packaging](/quest/m0/packaging.md).
- ✅ The `release` environment deploys from protected branches only
  (rejected: a `v*` tag rule now).
- ✅ Keep the Developer ID certificate in 1Password, expiring 2027-02-01
  (rejected: the G2 certificate `GX6RZQJ764`, whose key is not in 1Password).

Done 2026-11-08:

- `hang.live.p12` holds one certificate and its key: `Developer ID
  Application: Luke Curley (D7D5SDDB5Z)`, serial `4B518DF1A7AFBB24` (App Store
  Connect certificate `QMD585S65S`), the same certificate as
  `apple-developer-id`. It uses legacy (RC2/3DES) encryption, which macOS
  `security import` reads.
- The `release` environment exists, deployable only from protected branches,
  and holds `APPLE_CERTIFICATE` (the `.p12`, base64), `APPLE_API_KEY` (the key
  ID), and `APPLE_API_PRIVATE_KEY` (the `.p8` text).

Remaining, for an agent with the maintainer's `op` session (no secret passes
through chat or a file outside `.tmp/`):

- Store `APPLE_CERTIFICATE_PASSWORD` and `APPLE_API_ISSUER` (the issuer ID in
  the `apple-api` notes) in the `release` environment.
- Through the App Store Connect API with the `apple-api` key, register the
  macOS bundle ID `live.kixel.studio`, enable `SYSTEM_EXTENSION_INSTALL` on it, create a `MAC_APP_DIRECT`
  profile for it with certificate `QMD585S65S`, and store it base64 as
  `APPLE_PROVISIONING_PROFILE`. Then delete this quest.

## Waiting

- Maintainer: unlock 1Password on the Mac, or approve its CLI prompt. On
  2026-11-08 every `op read` from an agent failed with `error initializing
  client: authorization timeout` after the first few succeeded; the desktop
  app integration needs the maintainer's approval, which no agent can give.
  Check: `op read op://hang.live/apple-api/notesPlain >/dev/null` exits 0.
