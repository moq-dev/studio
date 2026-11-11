# [XS] The Apple signing identity is renewed

## Goal

Releases keep signing and notarizing past 2027-12-11, when the Developer ID
Application certificate `6WWG279G9M` and the "MoQ Studio Developer ID" profile
(`DXZWM6R3S2`, for `live.kixel.studio`) both expire.

## Plan

Decided 2026-11-11 (maintainer, /quest-merge pick of moq-dev/studio#112):
✅ keep this quest, with its deadline about two weeks before expiry
(rejected: drop it and renew from memory; another deadline).

Renewed 2026-11-11 (maintainer, /quest-unblock), replacing `QMD585S65S`,
which expires 2027-02-01. Apple issued the G2 certificate for 13 months, so
this repeats yearly; the PR that renews rewrites this quest for the next
expiry. The renewal that worked, all agent-driven in the maintainer's signed-in
Chrome:

- Generate an RSA 2048 key and CSR locally in `.tmp/`; no Mac is needed.
- Certificates > + > Developer ID Application (G2 Sub-CA), upload the CSR,
  download the `.cer`, and check it matches the key.
- Export a legacy-encrypted `.p12` (`-certpbe PBE-SHA1-3DES -keypbe
  PBE-SHA1-3DES -macalg sha1`, since macOS `security import` can reject
  OpenSSL 3's AES default) with a random password, and store it in 1Password
  `hang.live` as a new document item named for its expiry year, such as
  `apple-developer-id-2027`, with `password`, `certificate id`, and `expires`
  fields.
- Profiles > `DXZWM6R3S2` > Edit, select the new certificate, Save, and
  download it; the profile keeps its ID. Replace the file in
  `apple-developer-id-profile`.
- Replace `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, and
  `APPLE_PROVISIONING_PROFILE` in the `release` environment, then delete the
  local key material.

## Waiting

- Maintainer: before 2027-11-27, sign in to developer.apple.com in the agent Chrome (two-factor) and approve 1Password, so an agent can drive the renewal above; only the Account Holder can create a Developer ID certificate, and the `apple-api` key's role cannot. Check: `GET /v1/profiles/DXZWM6R3S2?include=certificates` with the `apple-api` key reports an expiration after 2027-12-11. (`/v1/certificates?filter[certificateType]=DEVELOPER_ID_APPLICATION` omits G2 certificates.)
