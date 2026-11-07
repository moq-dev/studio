# m2: later

## Goal

Work with no consumer yet, held until m0 and m1 ship or until something needs
it.

## Plan

Opened 2026-11-05 (maintainer, /quest-plan) for publishing a source
independently, which no service uses yet. A quest moves up to m0 or m1 when a
consumer appears.

Viewer-side composition moved here from m1 on 2026-11-07 (maintainer, on
[moq-dev/studio#97](https://github.com/moq-dev/studio/pull/97)): it composes
from sources published independently, so it follows that quest.

## Required

- [Published independently](/quest/m2/publish-independently.md) - a source opts out of the program and publishes its own track for services that place it themselves
- [Viewer-side composition](/quest/m2/compose/README.md) - independently published sources plus a layout track, composed by viewers and by a server
- [Overlays drop their Blitz panic guard](/quest/m2/overlay-blitz-release.md) - the next Blitz release lets overlay checks parse without `catch_unwind`
