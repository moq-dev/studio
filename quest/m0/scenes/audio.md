# [S] Pan in audio scenes

## Goal

Each sound's strip in the Audio panel has a pan beside its fader, saved as
`pan` in `[audio.<source>]` and applied in every audio scene holding the
sound, as gain, mute, and delay are. The rest of audio scenes landed in
[moq-dev/studio#77](https://github.com/moq-dev/studio/pull/77).

## Plan

Decided 2026-11-05 with the maintainer (/quest-plan); see the
[epic](/quest/m0/scenes/README.md).

- Pan is a property of the source's bus strip, like its gain, so it needs
  `moq_audio::bus::Strip` to pan; the studio only sets it and saves it.
- Changing pan never drops or repeats audio, ramped across a block as gain is.

## Waiting

- moq-dev/moq: `moq_audio::bus::Strip` gains a pan (with a moq-side test),
  and the studio's `moq` gitlink is bumped to a commit carrying it; check for
  a `set_pan` on `Strip` in the pinned `moq/rs/moq-audio/src/bus.rs`.
