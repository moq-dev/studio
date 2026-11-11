# [XS] moq-video's browser backends are pinned

## Goal

The `moq` submodule includes moq-video's browser backends (getUserMedia and
getDisplayMedia capture, WebCodecs, and VideoFrame import into wgpu) and the
wasm32 builds of hang and moq-mux, from
[moq-video in the browser](https://github.com/moq-dev/moq/blob/main/quest/m5/video-web.md).

## Plan

Once the wait clears, advance it with `just upgrade-moq` in its own PR,
fixing whatever the new API breaks here.

Moved to m2 with the [web studio](/quest/m2/web.md) on 2026-11-09
(maintainer, /quest-audit).

## Waiting

- moq-dev/moq: the upstream quest above lands. Check: that quest's PR has
  merged on moq's `main`, deleting its file, and moq-video's browser backends
  are there. A quest file that moved, or went away without its work landing,
  does not count.

## Required

- [The studio is caught up to moq's main](/quest/m0/pin-main.md) - moq's breaking changes since moq-dev/moq#8272, so this pin takes only its own
