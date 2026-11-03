# [XS] moq-audio's mix router is pinned

## Goal

The `moq` submodule includes moq-audio's multi-mix router (routing,
mix-minus, and per-source sync offsets), from
[moq-audio routes sources into several mixes](https://github.com/moq-dev/moq/blob/main/quest/m1/audio-mix-router.md),
which audio mixes configure.

Check: that quest file is gone from moq's `main`. Advance it with
`just upgrade-moq` in its own PR, fixing whatever the new API breaks here:
the single-mix `Bus` the program reads changes shape.
