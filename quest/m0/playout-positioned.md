# [M] Playout writes land on positioned bus channels

## Goal

A broadcast, media, or replay source's sound plays at exactly the bus
position its pictures are timed for. `rs/moq-scene`'s playout writes it
through moq-audio's `Bus::positioned`, and its copy of the live ring's pad
rule is deleted. How far ahead of its moment a sound is handed to the bus
stays a constant the studio owns: today's `BUS_LATENCY`, renamed for what it
is (such as `WRITE_LEAD`) and no longer a copy of moq-audio's private
`bus::LATENCY`.

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

Decided 2026-11-09 (maintainer, /quest-audit):

- ✅ **Keep a studio-owned write lead** (rejected: deleting `BUS_LATENCY`
  outright). The Goal deleted it while the `Speaker`'s targeting, a non-goal,
  is built on it: `Speaker::place` sounds a frame `target + BUS_LATENCY`
  after it arrives and re-buffers one that would follow less than half of it
  away, and `source.rs` hands each sound over that far ahead of its moment
  (`ahead`, the broadcast write loop, and where a media or replay play
  begins). A positioned write needs that lead too: the bus mixes a block at a
  time, and a write with any part already mixed fails. So the constant stays,
  within `bus::REACH`. What goes is the `Writer`'s pad rule and the claim
  that the constant is moq-audio's `bus::LATENCY`, which
  `a_write_sounds_when_placed` pins today.

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
