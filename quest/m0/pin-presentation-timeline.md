# [XS] moq-mux's presentation timeline is pinned

## Goal

The `moq` submodule includes moq-mux's audio-anchored playout timing, lifted
out of `moq play`, from
[moq-mux presents a broadcast on the publisher's timestamps](https://github.com/moq-dev/moq/blob/main/quest/m1/presentation-timeline.md),
which broadcast sources play on.

Check: that quest file is gone from moq's `main`. Advance it with
`just upgrade-moq` in its own PR, fixing whatever the new API breaks here.
