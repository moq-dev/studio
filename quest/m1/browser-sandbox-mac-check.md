# [XS] Check sandboxed browser sources in the signed macOS app

## Goal

The maintainer has run a browser source in the Developer ID signed,
notarized macOS app, installed from its disk image, and seen its page draw
with its renderer sandboxed, or the fix it needs is written into this Plan.

## Plan

[moq-dev/studio#223](https://github.com/moq-dev/studio/pull/223) sandboxes
each page's renderer on macOS. `macos.yml` runs the browser-source tests,
`a_renderer_cannot_read_the_user_s_files` among them, against an ad hoc
signed bundle with the release's hardened runtime and entitlements, but not
against the release's own signature, notarization, and quarantine, which a
download runs under (App Translocation moves a quarantined app to a random
read-only path, from which Chromium builds its sandbox's rules).

## Waiting

- Maintainer: on a Mac, download the next release's disk image (or a
  `release.yml` run's `macos-arm64` artifact) in a browser, open the app
  once from the image and once from Applications, add a browser source such
  as `https://webglsamples.org/aquarium/aquarium.html`, and check that it
  draws and plays, and that Activity Monitor's Sandbox column (View, Columns)
  says Yes for every `moq-studio Helper` process, plain ones included, since
  CEF 156 starts pages' renderers from the plain helper.
  No reachable Mac runs a signed app, and the release job runs nothing built
  until its signing credentials leave the runner; running the browser tests
  there was considered and would add a second hour-long build of the test
  harness to every release. Check: a comment or this Plan records the result.
