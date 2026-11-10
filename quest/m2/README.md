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

The web studio and its two pins moved here from m0 on 2026-11-09 (maintainer,
/quest-audit; rejected: keeping them in m0): moq removed all WASM support on
2026-10-31, its browser-backend quests are deferred with no other consumer,
and nothing plans a browser WebTransport session for moq-net.

## Required

- [A phone as a remote](/quest/m2/phone-remote.md) - scene pads, mutes, and tally on a phone from a QR code
- [Markers and clips](/quest/m2/markers.md) - markers on the show, clips around them, chapters in a recording
- [A command palette](/quest/m2/command-palette.md) - one search box that runs any action and finds any setting
- [Overlays drop their Blitz panic guard](/quest/m2/overlay-blitz-release.md) - the next Blitz release lets overlay checks parse without `catch_unwind`
- [Viewer-side composition](/quest/m2/compose/README.md) - players composite independently published sources over the program from a layer layout track
- [Browser sources paint on the GPU under Mesa AMD Wayland](/quest/m2/mesa-amd.md) - the Intel two-plane fix, checked on the `desktop` host's AMD GPU
- [Web studio](/quest/m2/web.md) - the full studio in wasm on WebGPU and WebCodecs, served from this repository
- [moq-video's browser backends are pinned](/quest/m2/pin-browser-video.md) - the web studio's video slice
- [moq-audio's browser backends are pinned](/quest/m2/pin-browser-audio.md) - the web studio's audio slice
