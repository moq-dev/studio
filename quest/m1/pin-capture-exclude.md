# [XS] moq-video's capture exclusion is pinned

## Goal

The `moq` submodule includes moq-video's display capture that leaves chosen
apps out. moq plans it for macOS alone, the one desktop whose capture can
filter, and an Unsupported error elsewhere.

## Plan

Planned 2026-11-09 (maintainer, /quest-plan) for
[the privacy shield](/quest/m1/privacy-shield.md). Advance it with
`just upgrade-moq` in its own PR.

## Waiting

- moq-dev/moq: [the capture exclusion quest](https://github.com/moq-dev/moq/blob/main/quest/m4/capture-exclude.md) lands on moq's `main`. Check that quest's PR has merged.

## Required

- [moq-mux's decoupled playout clock is pinned](/quest/m0/pin-sync.md) - no later pin can skip that commit
