# [XS] moq-audio's strip pan is pinned

## Goal

The `moq` submodule includes a pan on moq-audio's bus `Strip`, from
[Pan a bus source](https://github.com/moq-dev/moq/blob/main/quest/m3/audio-strip-pan.md).

## Plan

The pan landed upstream in
[moq-dev/moq#8180](https://github.com/moq-dev/moq/pull/8180)
(`Strip::set_pan` and `Strip::pan` on moq's `main`). Advance the gitlink with
`just upgrade-moq` in its own PR, fixing whatever the new API breaks here.
