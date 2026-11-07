# m2: later

## Goal

Work with no consumer yet, held until m0 and m1 ship or until something needs
it.

## Plan

Opened 2026-11-05 (maintainer, /quest-plan) for publishing a source
independently, which no service uses yet. A quest moves up to m0 or m1 when a
consumer appears.

## Required

- [Published independently](/quest/m2/publish-independently.md) - a source opts out of the program and publishes its own track for services that place it themselves
- [Overlays drop their Blitz panic guard](/quest/m2/overlay-blitz-release.md) - the next Blitz release lets overlay checks parse without `catch_unwind`
