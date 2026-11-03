# [XS] moq-video's program readback is pinned

## Goal

moq-video exposes its GPU readback (the `render::export` readback route:
staging buffers, row unpadding, the mapping lifecycle) without an encoder,
merged on moq's `main` with a moq-side test, and the `moq` gitlink here
includes it.
Check with `just upgrade-moq` and a search of `moq/rs/moq-video` for a public
readback-only constructor.

## Plan

Decided 2026-11-02 (maintainer, on
[#9](https://github.com/moq-dev/studio/pull/9)): the Linux virtual camera's
readback lives in moq-video, not in a copy here. The readback is private today
and reachable only through an encoder probe. Land the public API in
moq-dev/moq first; no moq quest tracks it yet. Bump the pin in its own PR,
fixing whatever the bump breaks.
