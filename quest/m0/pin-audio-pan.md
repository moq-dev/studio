# [XS] moq-audio's strip pan is pinned

## Goal

The `moq` submodule includes a pan on moq-audio's bus `Strip`, from
[Pan a bus source](https://github.com/moq-dev/moq/blob/main/quest/m3/audio-strip-pan.md).

## Plan

Once the wait clears, advance it with `just upgrade-moq` in its own PR,
fixing whatever the new API breaks here.

## Waiting

- moq-dev/moq: the upstream quest above lands. Check: that quest's PR has
  merged on moq's `main`, deleting its file, and `moq_audio::bus::Strip`
  there has a pan. A quest file that moved, or went away without its work
  landing, does not count. The quest itself is planned in
  [moq-dev/moq#8133](https://github.com/moq-dev/moq/pull/8133), not merged
  yet.
