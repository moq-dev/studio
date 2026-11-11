# [S] A lagging audio clock holds playout writes instead of failing the source

## Goal

When the program's audio clock thread falls behind the program clock, a
broadcast, media, or replay source's sound waits for the bus to catch up, or
loses only what the bus can no longer place, rather than failing the source.

## Plan

`playout::Writer` (`rs/moq-scene/src/playout.rs`) maps a moment to a bus
position from the program clock's start, and bounds each write by where the
clock says the bus is, since moq-audio's `Positioned` does not say where the
bus has mixed to. A write ends at most `WRITE_LEAD` plus one frame (about
70 ms) past the clock, inside `bus::REACH` (200 ms). If the audio thread lags
the clock by more than the rest (about 130 ms), `Positioned::write` refuses the
write as past its reach, with the same `Error::Unsupported` it gives a
non-finite sample, and the source fails: a media source stays failed until
restarted.

Recommended: moq-audio's `Positioned` reports the bus's next position (or
refuses a write past its reach with an error of its own), landed in moq-dev/moq
with a moq-side test and pinned here with `just upgrade-moq`. The writer then
cuts against the clock as now, but waits on the bus's actual position for
what lies past its reach.
