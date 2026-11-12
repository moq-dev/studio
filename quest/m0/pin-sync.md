# [M] moq-mux's decoupled playout clock is pinned

## Goal

The `moq` submodule is at `644d48996`,
[moq-dev/moq#8272](https://github.com/moq-dev/moq/pull/8272), which replaces
`moq_mux::timeline` with `moq_mux::sync`, and a broadcast source's playout
(`rs/moq-scene/src/playout.rs`, `rs/moq-scene/src/source.rs`) runs on the new
clock. `README.md`'s broadcast-source paragraph under "Keyframes" says what a
stall does on it.

## Plan

Decided 2026-11-09 (maintainer, /quest-audit): ✅ crossing #8272 is this
quest, after playout's writes moved onto positioned bus channels
([moq-dev/studio#137](https://github.com/moq-dev/studio/pull/137))
(rejected: crossing it in the positioned bus pin). Both rewrite
`playout.rs`, so the positioned work finishes on the API it was planned
against, then this crosses.

Advance with `just upgrade-moq 644d48996` in its own PR. #8272's Impact holds
the migration:

- `timeline::Kind` is `moq_mux::container::Kind`, which `Producer::track`
  takes.
- `Producer::video(ts, now)` is `track.received(ts, now)` on the video
  `Track`. `Producer::audio(Audio { .. }, now)` is `track.received(ts,
  arrival)` on the audio `Track` plus `track.set_target(jitter_target)`, and
  `timeline::Audio` is gone.
- `video_restarted` and `audio_restarted` are `track.reset()`;
  `audio_stopped` is dropping or retiring the track.
- `Consumer::target()` is `Consumer::delay()`, always resolved.
  `Consumer::playhead()` is gone, which `playout::heard` reads: the speaker
  tracks what it sounds.
- Audio no longer owns the clock. Each frame sounds at `producer.due(..)`,
  which the `Speaker` steers toward itself.

The other breaking changes in `79960a9dd..644d48996` that reach studio code,
read against the tree on 2026-11-09; `just ci` finds any other:

- [moq-dev/moq#8274](https://github.com/moq-dev/moq/pull/8274):
  `mix.minus(strip)` takes every strip to remove, `mix.minus([&strip])`
  (`rs/moq-studio/src/feed.rs`).
- [moq-dev/moq#8269](https://github.com/moq-dev/moq/pull/8269):
  `moq_video::encode::Producer::new` takes a track, not a broadcast, and
  `with_track` is gone; `render::Exporter::new` takes `&encode::Config` and
  returns `(Exporter, encode::Config, VideoConfig)`.

Not crossed here: [moq-dev/moq#8270](https://github.com/moq-dev/moq/pull/8270)
(`241d2ce92`), [moq-dev/moq#8372](https://github.com/moq-dev/moq/pull/8372)
(`147327e14`), and
[moq-dev/moq#8220](https://github.com/moq-dev/moq/pull/8220) (`16a673036`,
kio's `Instant` loses `+` and `-` with a `Duration`, which `playout.rs` and
`source.rs` use throughout) all merged after #8272.
[pin-main](/quest/m0/pin-main.md) fixes them.

Decided 2026-11-12 (maintainer, /quest-unblock): ✅ adopt moq's decoupled
rule for a sound stall (rejected: keeping the studio's rule, with a `Speaker`
playhead of its own that holds the picture after a stall). The picture keeps
playing through a sound stall, and the sound comes back on the clock: each
frame is written at the bus position `due` gives it, and the late part of a
burst is trimmed, as the positioned writer already does. The studio plays a
broadcast as `moq play` and the browser do, following the 2026-11-05 ruling on
[moq-dev/moq#7941](https://github.com/moq-dev/moq/pull/7941): lip sync in the
steady state, desync under congestion, and one slow track never stalls or
moves another. `README.md` drops "Each picture waits for the sound stamped
with it" and "A sound stall longer than that delay pauses the picture too and
starts both again together", and says the picture plays on through a stall.
