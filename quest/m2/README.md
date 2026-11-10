# m2: later

## Goal

Work with no consumer yet, held until m0 and m1 ship or until something needs
it.

## Plan

Opened 2026-11-05 (maintainer, /quest-plan). A quest moves up to m0 or m1
when a consumer appears. Publishing a source independently and viewer-side
composition moved on to [m3](/quest/m3/README.md) on 2026-11-07 (maintainer):
no customer asks for them. They came back to m2 on 2026-11-08 (maintainer,
/quest-plan), replanned around players that composite independent sources
over the program. Publishing a source independently shipped the same day
([moq-dev/studio#117](https://github.com/moq-dev/studio/pull/117)).

## Required

- [Overlays drop their Blitz panic guard](/quest/m2/overlay-blitz-release.md) - the next Blitz release lets overlay checks parse without `catch_unwind`
- [Viewer-side composition](/quest/m2/compose/README.md) - players composite independently published sources over the program from a layer layout track
- [Browser sources paint on the GPU under Mesa AMD Wayland](/quest/m2/mesa-amd.md) - the Intel two-plane fix, checked on AMD once a host exists
