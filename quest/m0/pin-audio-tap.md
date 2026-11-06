# [XS] moq-audio's source tap is pinned

## Goal

The `moq` submodule includes moq-audio's tap of a bus source's own samples,
before its delay and stamped with their bus-clock positions, from
[Read a bus source's own samples](https://github.com/moq-dev/moq/blob/main/quest/m3/audio-strip-tap.md).

Check: that quest file is gone from moq's `main`. Advance it with
`just upgrade-moq` in its own PR, fixing whatever the new API breaks here.
