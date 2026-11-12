# [XS] Platform AAC encoders are pinned

## Goal

The `moq` submodule includes moq-audio's platform AAC encoders, from
[AudioToolbox AAC encode](https://github.com/moq-dev/moq/blob/main/quest/m3/audio-encode-audiotoolbox.md) on macOS and
[Media Foundation AAC encode](https://github.com/moq-dev/moq/blob/main/quest/m3/audio-encode-mediafoundation.md) on
Windows, which destinations whose ingest requires AAC use.

## Plan

Once the wait clears, advance it with `just upgrade-moq` in its own PR,
fixing whatever the new API breaks here.

## Waiting

- moq-dev/moq: both upstream quests above land. Check: both quests' PRs have
  merged on moq's `main`, deleting their files, and moq-audio's AAC encoders
  are there. A quest file that moved, or went away without its work landing,
  does not count.

## Required

- [The studio is caught up to moq's main](/quest/m0/pin-main.md) - moq's breaking changes since moq-dev/moq#8272, so this pin takes only its own
