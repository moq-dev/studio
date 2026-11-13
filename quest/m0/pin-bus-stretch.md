# [S] A broadcast source's sound glides onto a moved delay

## Goal

The `moq` submodule includes moq-audio's stretching `Positioned` channel, from
[The bus stretches positioned audio onto its clock](https://github.com/moq-dev/moq/blob/main/quest/m1/audio-jitter-target/bus-stretch.md),
and a broadcast source's playout (`rs/moq-scene/src/playout.rs`) writes
through it with a stretch bound, so its sound glides onto a moved playout
delay as `moq play`'s does. A step of the jitter target no longer writes
silence into the program's sound or skips it. `README.md`'s broadcast-source
paragraph says how a delay change sounds.

## Plan

Decided 2026-11-12 (maintainer, /quest-plan), after
[moq-dev/studio#144](https://github.com/moq-dev/studio/pull/144) kept a
target step's gap or skip (its follow-up): moq-audio's bus stretches, since
audio mixing is moq's (`AGENTS.md`'s Upstream), and this quest pins it. ✅ m0,
beside the other pins (rejected: m2).

Advance with `just upgrade-moq` in its own PR, fixing whatever the new API
breaks here. Take the bound `moq play` defaults to (2 s, its `--stretch`)
unless a measurement here argues for another, and say which in the PR.

## Waiting

- moq-dev/moq: the upstream quest above lands. Check: that quest's PR has
  merged on moq's `main`, deleting its file, and `moq_audio::bus::Positioned`
  there takes a stretch bound. A quest file that moved, or went away without
  its work landing, does not count.

## Required

- [The studio is caught up to moq's main](/quest/m0/pin-main.md) - moq's breaking changes since moq-dev/moq#8272, so this pin takes only its own
