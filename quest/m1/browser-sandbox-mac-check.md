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

Checked 2026-11-25 (/quest-run, agent on the maintainer's Mac, macOS 27.2,
v0.0.1 arm64 image, which `spctl` reads as Notarized Developer ID):

- Unquarantined, not translocated, launched with a scene file holding a
  `kind = "browser"` source for the aquarium page: every `moq-studio Helper`
  was sandboxed (GPU, network, storage, and both renderers), all started from
  the plain `moq-studio Helper.app`. The main process and
  `--moq-browser-helper` were not, as Chromium expects for the browser
  process. Read with libSystem's `sandbox_check(pid, NULL, 0)`, which
  Activity Monitor uses, checked first against known sandboxed and
  unsandboxed processes. The page's drawing was not seen (the agent has no
  Screen Recording grant), only its connection to webglsamples.org and
  steady renderer and GPU load.
- Quarantined, from the image and from a copy in `/Applications`: both were
  translocated and stopped at Gatekeeper's first-launch prompt, which only a
  person can answer, so no helper started. A copy made outside Finder stays
  translocated.
- Ending the studio with SIGTERM made `--moq-browser-helper` abort
  (SIGABRT, `abort() called`) both times, leaving crash reports. A normal
  quit was not tried; check it in the same sitting, and if Cmd-Q crashes it
  too, the fix belongs in this Plan.

## Waiting

- Maintainer: on a Mac, download the
  [latest release](https://github.com/moq-dev/studio/releases/latest)'s disk
  image (v0.0.1 or later) in a browser, open the app
  once from the image and once after dragging it to Applications in Finder,
  clicking Open on Gatekeeper's prompt, add a browser source such
  as `https://webglsamples.org/aquarium/aquarium.html`, and check that it
  draws and plays, and that Activity Monitor's Sandbox column (View, Columns)
  says Yes for every `moq-studio Helper` process, plain ones included, since
  CEF 156 starts pages' renderers from the plain helper; then quit with Cmd-Q
  and check `~/Library/Logs/DiagnosticReports` gains no `moq-studio` report.
  Only the quarantined, translocated launches are left: an agent cannot
  answer Gatekeeper's prompt. The release job runs nothing built
  until its signing credentials leave the runner; running the browser tests
  there was considered and would add a second hour-long build of the test
  harness to every release. Check: a comment or this Plan records the result.
