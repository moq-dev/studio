# [XS] The Apple signing identity is renewed

## Goal

Releases keep signing and notarizing past 2027-12-25, when the Developer ID
Application certificate `9ZNCPSRVNL` and the "MoQ Studio Developer ID" profile
(`DXZWM6R3S2`, for `live.kixel.studio`) both expire.

## Plan

Decided 2026-11-11 (maintainer, /quest-merge pick of moq-dev/studio#112):
✅ keep this quest, with its deadline about two weeks before expiry
(rejected: drop it and renew from memory; another deadline).

Renewed 2026-11-11 (replacing `QMD585S65S`) and again 2026-11-25
(maintainer, /quest-run, replacing `6WWG279G9M`, which expires 2027-12-11).
Apple issues the G2 certificate for 13 months, so this repeats yearly; the PR
that renews rewrites this quest for the next expiry.

Decided 2026-11-25 (maintainer, /quest-run): ✅ leave superseded Developer ID
certificates unrevoked; they expire on their own (rejected: revoking
`QMD585S65S`). Leave `97F79J652T` alone too: it came from outside this quest,
likely for another project on the same team.

The renewal that worked, all agent-driven in the maintainer's signed-in
Chrome:

- The agent Chrome runs on `fedora` behind an SSH tunnel, so files it uploads
  or downloads live there: `scp` the CSR to `fedora` before uploading it and
  copy the `.cer` back. The profile's Download button hangs `agent-browser
  download`; `fetch` its `href` in the page instead.
- When the maintainer is at `fedora`, run `op` there (`ssh -o BatchMode=yes
  fedora op ...`): prompts on other hosts are dismissed. Pass secrets through
  `scp` into a `mktemp -d` there, never on a command line, and delete it after.
- Generate an RSA 2048 key and CSR locally in `.tmp/`; no Mac is needed.
- Certificates > + > Developer ID Application (G2 Sub-CA), upload the CSR,
  download the `.cer`, and check it matches the key.
- Export a legacy-encrypted `.p12` (`-certpbe PBE-SHA1-3DES -keypbe
  PBE-SHA1-3DES -macalg sha1`, since macOS `security import` can reject
  OpenSSL 3's AES default) with a random password, and store it in 1Password
  `hang.live` named for its expiry date, such as
  `apple-developer-id-2027-12-25`: a Secure Note created with `op item create
  --template` holding `password`, `certificate id`, and `expires` fields, then
  the `.p12` attached with `op item edit` (`name\.p12[file]=path`; escape the
  dot). `op` drops custom fields from a Document item's template edit, and an
  assignment would put the password on a command line.
- Profiles > `DXZWM6R3S2` > Edit, select the new certificate, Save (a DOM
  `click()`; the agent-browser click does nothing), and download it; the
  profile keeps its ID. Replace the file in `apple-developer-id-profile`.
- Replace `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, and
  `APPLE_PROVISIONING_PROFILE` in the `release` environment, run the check
  below, then delete the local key material.

## Waiting

- Maintainer: before 2027-12-11, sign in to developer.apple.com in the agent Chrome (two-factor) and approve 1Password, so an agent can drive the renewal above; only the Account Holder can create a Developer ID certificate, and the `apple-api` key's role cannot. Check: `GET /v1/profiles/DXZWM6R3S2?include=certificates` with the `apple-api` key reports an expiration after 2027-12-25. (`/v1/certificates?filter[certificateType]=DEVELOPER_ID_APPLICATION` omits G2 certificates.)
