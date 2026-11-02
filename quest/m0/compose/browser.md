# [L] Browser composition

## Goal

A browser player composes a broadcast that carries a layout track: it fetches
the tracks the layout shows and draws them through moq-scene's browser build,
matching the studio's picture. A broadcast without one plays its program as
`@moq/watch` does today.

## Plan

The browser build comes from whichever line
[JS-only @moq/net from Rust](https://github.com/moq-dev/moq/blob/main/quest/m2/rs2ts-js.md) lands (decided 2026-10-26; moq's WASM
support was removed on 2026-10-31), so this quest does not fix the
mechanism. A thin package published from this repository (e.g. `@moq/scene`)
wraps it and layers on `@moq/watch`; `@moq/watch` never depends on it, so moq
never depends on this repository (decided 2026-11-02, when the studio left
moq). Frames reach it as WebCodecs `VideoFrame`s through the browser import
path [moq-video in the browser](https://github.com/moq-dev/moq/blob/main/quest/m3/video-web.md) adds.

Prove parity with a fixture: the same broadcast composited here and by the
studio, compared within a stated tolerance.

## Required

- [Layout track](/quest/m0/compose/layout.md) - what the browser composes
- [moq-video's browser backends are pinned](/quest/m0/pin-browser-video.md) - browser frames into wgpu
