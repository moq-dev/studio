# [XS] moq-audio's filter chain is pinned

## Goal

The `moq` submodule includes moq-audio's per-source filter chain and plugin
host, from [moq-audio filter chain](https://github.com/moq-dev/moq/blob/main/quest/m3/audio-filters.md).

## Plan

Once the wait clears, advance it with `just upgrade-moq` in its own PR,
fixing whatever the new API breaks here.

## Waiting

- moq-dev/moq: the upstream quest above lands. It moves to moq's m3 in
  [moq-dev/moq#8421](https://github.com/moq-dev/moq/pull/8421), so the link
  resolves once that merges. Check: that quest's PR has
  merged on moq's `main`, deleting its file, and moq-audio's filter chain is
  there. A quest file that moved, or went away without its work landing, does
  not count.

## Required

- [The studio is caught up to moq's main](/quest/m0/pin-main.md) - moq's breaking changes since moq-dev/moq#8272, so this pin takes only its own
