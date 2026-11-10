# [XS] moq-video's capture listing is pinned

## Goal

The `moq` submodule includes moq-video's richer capture listing: each window
and app with its icon and a stable identity, a live thumbnail of a window,
display, or camera without adding it as a source, and a notice when the list
changes.

## Plan

Planned 2026-11-09 (maintainer, /quest-plan) for the
[Add page](/quest/m0/ux/add.md), which shows pictures where the Devices panel
shows names. Advance it with `just upgrade-moq` in its own PR, fixing whatever
the new API breaks in `rs/moq-studio/src/devices.rs`.

## Waiting

- moq-dev/moq: [the capture listing quest](https://github.com/moq-dev/moq/blob/main/quest/m3/capture-list.md) lands on moq's `main`. Check that quest's PR has merged.

## Required

- [moq-mux's decoupled playout clock is pinned](/quest/m0/pin-sync.md) - no later pin can skip that commit
