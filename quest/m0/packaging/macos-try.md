# [XS] The maintainer tries the macOS app

## Goal

The maintainer has run the macOS app from its disk image on a Mac with a
camera and seen the camera's picture in program, and the first release's
`Sign` jobs in `.github/workflows/macos.yml` have signed, notarized, and
stapled both the app and its disk image as the real Developer ID.

## Plan

Decided 2026-11-15 (maintainer, on moq-dev/studio#170): ✅ these checks
are this follow-up's, so the macOS app's PR lands without them (rejected:
keeping that PR a draft until the hand check is done).

CI covers everything else. A pull request that touches `packaging/macos/`,
and Nightly, build each architecture's disk image, sign the app ad hoc, and
launch it from the mounted image with no camera. A pull request cannot deploy
the `release` environment, so the Developer ID signing and both notarizations
first run in a release.

Once a release run exists, an agent checks its `Sign (arm64)` and
`Sign (x86_64)` jobs passed: `Notarize the app` and `Notarize the image`
print `"status": "Accepted"`, and `Verify` prints `source=Notarized Developer
ID` for both. If one failed, it fixes the workflow in its own PR, linked here.

## Hosts

- macos - a Mac with a camera, and a person at it

## Waiting

- Maintainer: on a Mac with a camera, open the app from its disk image, approve the camera prompt, pick the camera in guided setup, and check its picture shows in program. Only a person can grant the camera permission (TCC has no scriptable grant outside MDM), and hosted runners have no camera. Take `moq-studio-macos-arm64.dmg` (or `-x86_64`) from the latest release, which is signed and needs no other step; before the first release, download the `unsigned-macos-arm64` artifact from the latest [Nightly](https://github.com/moq-dev/studio/actions/workflows/nightly.yml) run, open the disk image, drag the app to Applications, and run `xattr -dr com.apple.quarantine "/Applications/kixel.live Studio.app"`, since it is signed ad hoc. Check: the maintainer comments on this quest's PR, or edits this item away, with what they saw.
- Maintainer: run `.github/workflows/release.yml` on main to cut the first release with the macOS app, since a release publishes a version only the maintainer chooses. Check: a GitHub release lists `moq-studio-macos-arm64.dmg` and `moq-studio-macos-x86_64.dmg`.

## Related

- [Studio packaging](/quest/m0/packaging/README.md) - the release workflow the app's images ship from
