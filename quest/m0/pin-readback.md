# [XS] moq-video's program readback is pinned

## Goal

The `moq` submodule includes moq-video's public readback, which copies a
wgpu texture into packed RGBA with no encoder, from
[Read a texture back without an encoder](https://github.com/moq-dev/moq/blob/main/quest/m1/video-readback.md).

Check: that quest's PR has merged on moq's `main`, deleting its file, and
`moq/rs/moq-video` there has a public readback with no encoder probe. A quest
file that moved, or went away without its work landing, does not count.
Advance it with `just upgrade-moq` in its own PR, fixing whatever the new API
breaks here.

## Plan

Decided 2026-11-02 (maintainer, on
[#9](https://github.com/moq-dev/studio/pull/9)): the Linux virtual camera's
readback lives in moq-video, not in a copy here.
