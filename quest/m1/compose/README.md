# Viewer-side composition

## Goal

A `moq-studio` broadcast carries each source as its own track plus a layout
track describing the live scene. Viewers compose the picture themselves:
they prioritize sources and toggle elements on and off. A composited program
remains for protocols and devices that cannot compose (RTMP and WHIP
destinations, HLS, legacy players), produced by the studio or by a server.

This README's own work is the end-to-end test: a studio broadcast is composed
by the browser, by the headless compositor, and by the studio itself, and all three
pictures match.

## Plan

Decided 2026-10-26 (maintainer), the studio's flagship and first in its epic:

- **Delivery:** MoQ's value is per-track delivery, so the studio embraces
  viewer composition. The program is the fallback, not the default.
- **One compositor, three hosts:** moq-scene renders in the studio, headless
  on a server (the headless compositor), and in the browser, so all three
  pictures come from one implementation.
- **Parity rule:** every scene feature (transitions, filters, effects,
  overlays) renders in all three hosts with the same layout, timing, and
  colors, within a tolerance. Each renderer keeps its own text antialiasing
  and blur, so the browser can draw overlays as real DOM. A feature that
  cannot meet this, such as a CEF browser source or an ML effect, is baked
  into its source's own track before publishing rather than living in the
  layout.
- **Wire layout:** local sources are tracks in the studio's one broadcast.
  Guests stay their own broadcasts, named through the hang catalog's
  `contributions` references.
- **Layout:** the live scene state (items, transforms, transitions, and each
  element's priority and toggleable flag) as a hang JSON track on the media
  clock, so any host composites the same frame the studio shows.
- **Browser build:** whichever line [JS-only @moq/net from Rust](https://github.com/moq-dev/moq/blob/main/quest/m2/rs2ts-js.md)
  lands. moq's WASM support was removed on 2026-10-31.
- **Cost:** every source and the program encode whatever the subscriber
  demand (decided 2026-10-28, from
  [#7101](https://github.com/moq-dev/moq/issues/7101)), so a late viewer or a
  replay finds their recent past. Upload follows MoQ's pull model, so a
  fallback nobody uses costs its encode but no bandwidth.

Changed 2026-11-04 (maintainer): this epic moved to m1, and the program is
the default, not the fallback. Encoding every source for viewers would take
more encoder sessions than a consumer GPU has (decided 2026-11-03 on
[moq-dev/studio#21](https://github.com/moq-dev/studio/pull/21)), so sources
are not published to the relay by default: a source's own track encodes only
while something in the studio uses it, such as an isolated recording or a
replay, and reaches viewers only when the creator ticks
[Published independently](/quest/m0/publish-independently.md). The Delivery
and Cost bullets above stand only for sources published that way; replan them
when this epic starts.

## Required

- [Audio source tracks](/quest/m0/audio-source-tracks.md) - microphones and system audio get their own tracks beside the picture sources (an m0 quest), encoded while the studio uses them and published to the relay only when published independently
- [Layout track](/quest/m1/compose/layout.md) - the live scene state as a hang JSON track on the media clock, specified in the hang draft
- [Browser composition](/quest/m1/compose/browser.md) - `@moq/watch` composes a broadcast with a layout track through moq-scene's browser build
- [Server composition](/quest/m1/compose/server.md) - a headless compositor renders the program from sources and layout, headless
- [Viewer controls](/quest/m1/compose/controls.md) - viewers toggle elements and the layout's priorities steer what they fetch
