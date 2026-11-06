# [XS] moq-audio's source tap is pinned

## Goal

The `moq` submodule includes moq-audio's tap of a bus source's own samples,
before its delay and stamped with their bus-clock positions, from
[Read a bus source's own samples](https://github.com/moq-dev/moq/blob/main/quest/m3/audio-strip-tap.md).

## Plan

Once the wait clears, advance it with `just upgrade-moq` in its own PR,
fixing whatever the new API breaks here.

## Waiting

- moq-dev/moq: the upstream quest above lands. Check: that quest's PR has
  merged on moq's `main`, deleting its file, and `moq_audio::bus` there offers
  a tap of a source's samples with bus positions. A quest file that moved, or
  went away without its work landing, does not count.
