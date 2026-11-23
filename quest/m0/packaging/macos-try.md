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

Checked 2026-11-23 (/quest-run): the
[v0.0.1 release run](https://github.com/moq-dev/studio/actions/runs/45101688958)'s
`Sign (arm64)` and `Sign (x86_64)` jobs passed: both notarizations printed
`"status": "Accepted"`, both staples worked, and `Verify` printed
`source=Notarized Developer ID` for the app and the image. The first run had
failed with empty signing secrets, fixed by
[moq-dev/studio#234](https://github.com/moq-dev/studio/pull/234).

## Hosts

- macos - a Mac with a camera, and a person at it

## Waiting

- Maintainer: on a Mac with a camera, open the app from its disk image, approve the camera prompt, click the quick start's Camera card, and check its picture shows in program. Only a person can grant the camera permission (TCC has no scriptable grant outside MDM), and hosted runners have no camera. Take `moq-studio-macos-arm64.dmg` (or `-x86_64`) from the [latest release](https://github.com/moq-dev/studio/releases/latest), which is signed and needs no other step. Check: the maintainer comments on this quest's PR, or edits this item away, with what they saw.
- Maintainer: on macOS, check the window's top-left, launching with no arguments, with a scene file (`moq-studio show.toml`), and after switching shows from the Show menu. The window is always borderless with its content under the traffic lights, so check they clear the show's name, and that dragging the title bar, or the "Opening…" page while a show opens, moves the window. No reachable Mac host can run this, and the Settings page (moq-dev/studio#187), which also uses the title bar, should be tried in the same sitting. Check: the maintainer comments on this quest's PR, or edits this item away, with what they saw.

## Related

- [Studio packaging](/quest/m0/packaging/README.md) - the release workflow the app's images ship from
