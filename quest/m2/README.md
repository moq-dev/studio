# m2: later

## Goal

Work with no consumer yet, held until m0 and m1 ship or until something needs
it.

## Plan

Opened 2026-11-05 (maintainer, /quest-plan). A quest moves up to m0 or m1
when a consumer appears. Publishing a source independently and viewer-side
composition moved on to [m3](/quest/m3/README.md) on 2026-11-07 (maintainer):
no customer asks for them.

## Required

- [Overlays drop their Blitz panic guard](/quest/m2/overlay-blitz-release.md) - the next Blitz release lets overlay checks parse without `catch_unwind`
