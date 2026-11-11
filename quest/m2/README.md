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
