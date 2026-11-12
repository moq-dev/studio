# [L] The studio is caught up to moq's main

## Goal

The `moq` submodule is at moq's `main` as of the day this quest starts, and
`just ci` passes on it. Every breaking change between `644d48996`
([moq-dev/moq#8272](https://github.com/moq-dev/moq/pull/8272), where the
studio's pin sits) and that commit is fixed in one
PR, so each later wait-gated pin moves past only its own upstream change.

## Plan

Decided 2026-11-09 (maintainer, /quest-plan on the follow-up from
[moq-dev/studio#123](https://github.com/moq-dev/studio/pull/123)):

- ✅ Pin `main` on the day it starts, with `just upgrade-moq` and no commit
  argument (rejected: a commit fixed at planning time; dropping the quest
  and letting the first small pin absorb the catch-up). A commit chosen now
  goes stale before anyone starts the quest.
- ✅ Every pin whose upstream work lands after pin-sync requires this quest
  (rejected: leaving them on pin-sync). Each then stays XS.

On 2026-11-09, moq's `main` (`103052ed0`) was 117 first-parent commits past
`644d48996`, 27 of them breaking. The ones that reach studio code most:

- [moq-dev/moq#8220](https://github.com/moq-dev/moq/pull/8220): kio's
  `Instant` has no `+` or `-` with a `Duration`. `rs/moq-scene/src/playout.rs`
  and `source.rs` use them throughout.
- [moq-dev/moq#8270](https://github.com/moq-dev/moq/pull/8270): moq-audio
  codecs return zero or more outputs and flush.
- [moq-dev/moq#8372](https://github.com/moq-dev/moq/pull/8372): moq-video
  codecs split submission from receive.
- Renames and surface cuts in moq-net, moq-tokio, moq-mux, and moq-video
  (#8247, #8265, #8266, #8298, #8305, #8369, #8370, #8371, #8375, #8392).
  `just ci` finds the rest.

Each change's PR has an Impact section with the migration. If an upstream
change a pin waits for is already on `main` when this starts, this PR also
does that pin's work and deletes the pin quest.
