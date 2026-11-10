# [XS] moq-video's foreground app is pinned

## Goal

The `moq` submodule includes moq-video's report of which app and window are
in front, with a notice when that changes, on the desktops that tell an app.

## Plan

Planned 2026-11-09 (maintainer, /quest-plan) for
[a source that follows the game](/quest/m1/follow-game.md). Advance it with
`just upgrade-moq` in its own PR.

## Waiting

- moq-dev/moq: [the foreground app quest](https://github.com/moq-dev/moq/blob/main/quest/m3/capture-foreground.md) lands on moq's `main`. Check that quest's PR has merged.

## Required

- [moq-mux's decoupled playout clock is pinned](/quest/m0/pin-sync.md) - no later pin can skip that commit
