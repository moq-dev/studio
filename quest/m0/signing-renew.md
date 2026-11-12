# [XS] The Apple signing identity is renewed

## Goal

Releases keep signing and notarizing past 2027-02-01, when the Developer ID
Application certificate `QMD585S65S` and the "MoQ Studio Developer ID" profile
(`DXZWM6R3S2`, for `live.kixel.studio`) both expire.

## Plan

Decided 2026-11-11 (maintainer, /quest-merge pick of moq-dev/studio#112):
✅ keep this quest, with the 2027-01-15 deadline about two weeks before expiry
(rejected: drop it and renew from memory; another deadline).

Once the maintainer has the new certificate, an agent exports it as a
legacy-encrypted `.p12` (macOS `security import` can reject OpenSSL 3's AES
default), regenerates the profile through the App Store Connect API with the
new certificate, stores both in 1Password `hang.live` (items
`apple-developer-id` and `apple-developer-id-profile`), and replaces
`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, and
`APPLE_PROVISIONING_PROFILE` in the `release` environment.

## Waiting

- Maintainer: before 2027-01-15, create a new Developer ID Application certificate at developer.apple.com > Certificates, since only the Account Holder can and the `apple-api` key's role cannot (`POST /v1/bundleIds` answered 403 on 2026-11-08); attach its `.p12` to the `apple-developer-id` item in 1Password `hang.live`. Check: `GET /v1/certificates?filter[certificateType]=DEVELOPER_ID_APPLICATION` with the `apple-api` key lists one expiring after 2027-02-01.
