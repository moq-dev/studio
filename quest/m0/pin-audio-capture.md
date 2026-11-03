# [XS] One audio capture feeding the bus and an encoder is pinned

## Goal

The `moq` submodule includes moq-audio's capture Producer/Consumer split, from
[One audio capture feeds the bus and an encoder](https://github.com/moq-dev/moq/blob/main/quest/m1/audio-capture-fanout.md),
which the studio's microphone and system-audio source tracks need.

Check: that quest file is gone from moq's `main`. Advance it with
`just upgrade-moq` in its own PR, fixing whatever the new API breaks here:
`bus::Capture` and `encode::publish_capture` are replaced by the split.
