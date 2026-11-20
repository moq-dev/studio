# [S] The composition broadcast carries MSF beside hang

## Goal

The composition broadcast publishes its catalog through moq-mux's catalog
producer, so it carries MSF beside hang, each update still stamped with the
frame it applies at.

## Plan

[moq-dev/studio#146](https://github.com/moq-dev/studio/pull/146) publishes the
composition through a hang-only `moq_json::snapshot::Producer`, calling
`Timed::new` itself, because moq-mux's producer stamps each emit on its own
clock. Once the wait clears, advance the gitlink with `just upgrade-moq` in
this PR unless a bump already has, and move the composition onto
`moq_mux::catalog::Producer` with the caller's timestamp.

## Waiting

- moq-dev/moq: moq-mux's catalog producer publishes at the caller's
  timestamp, as
  [The catalog producer publishes at the caller's timestamp](https://github.com/moq-dev/moq/blob/main/quest/m1/catalog-timestamp.md)
  plans (being planned in [moq-dev/moq#9247](https://github.com/moq-dev/moq/pull/9247),
  so the link resolves once it merges). Check: that quest's PR has merged on
  moq's `main`, deleting its file. A quest file that moved, or went away
  without its work landing, does not count.

## Required

- [The studio catches up to moq's main](/quest/m0/pin-moq-tip.md) - absorbs the tip's other breaks, so this bump moves only by the producer
