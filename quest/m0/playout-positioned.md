# [M] Playout writes land on positioned bus channels

## Goal

A broadcast, media, or replay source's sound plays at exactly the bus
position its pictures are timed for. `rs/moq-scene`'s playout writes it
through moq-audio's `Bus::positioned`, and its private copy of moq-audio's bus
latency (`BUS_LATENCY`) and of the live ring's pad rule are deleted.

Non-goals: the `Speaker`'s jitter targeting stays as it is, and live inputs
(microphones, capture, guests) stay on `Bus::live`.

## Plan

Planned 2026-11-08 (maintainer, moq's /quest-complete), a follow-up of
[moq-dev/moq#8273](https://github.com/moq-dev/moq/pull/8273), which added
`Bus::positioned` for this consumer.

Today `playout::Writer` writes into a live `bus::Channel` and keeps track of
when what it wrote runs out, padding silence ahead of a frame so it sounds a
copied 50 ms `BUS_LATENCY` later, the way moq-audio's ring would. Each source
that plays timed audio (`source.rs`'s broadcast, media, and replay paths)
goes through it.

Decided with the maintainer:

- ✅ **Studio maps an instant to a bus position** (rejected: an upstream
  moq-audio bus clock). `Program::run_audio` (`rs/moq-studio/src/engine.rs`) already advances the bus
  sample-exact on the program clock from `start`, so the position a frame
  sounds at is `(sounds - start) * rate`. The writer gets the program clock's
  `start` and the bus rate through the sources' `Context`. moq-audio's bus
  counts frames and has no wall clock, so a clock there would guess timing
  only the app knows.
- ✅ **A late frame is trimmed, not dropped** (rejected: drop the whole
  frame). `Positioned::write` fails a write any part of which is already
  mixed, overlaps earlier audio, or lands past `bus::REACH`. The writer cuts
  the frames whose position has passed and writes the rest at its position,
  as today's writer does with a late part.

Recommendations, not settled:

- Trim against the later of the already-mixed position and the previous
  write's end, so a frame overlapping queued audio writes its playable tail
  instead of failing whole.
- Keep consecutive writes contiguous: a frame that starts within the
  existing one-millisecond `CONTIGUOUS` stamp rounding of where the last one
  ended is written at that end, since a one-frame overlap from rounding fails
  the write. The snap only absorbs stamp rounding: confirm the source's stamp
  resolution and shrink the tolerance to it (e.g. a sample or two) if that is
  tighter, so a genuine sub-millisecond gap stays silent.
- A frame the writer holds earlier than `REACH` ahead waits until it is in
  reach rather than failing.
- Test on the scene's driver-stepped bus, as `a_write_sounds_when_placed`
  does: frames written ahead, late, and across a stall sound at the expected
  bus positions, sample-exact.

## Required

- [moq-audio's positioned bus channel is pinned](/quest/m0/pin-bus-positioned.md) - `Bus::positioned`, which the writer writes through
