# [S] Removing a scene passes every time under load

## Goal

`shots::a_scene_removes_from_its_pad` passes every time in a full
`just shots` run on a loaded host, fixed at the root cause: either the test
acts before a frame or the engine catches up, or removing a scene has a real
race. No retry, sleep, or longer timeout. The other shots tests are out of
scope.

## Plan

Planned 2026-11-26 (maintainer, /quest-run follow-up): ✅ the goal above
(rejected: auditing every shots test, which has no evidence yet; dropping it
until it fails more). ✅ m0, since a flaky `just shots` costs every PR in the
milestone a rerun and an agent turn.

What is known: the test came in with
[moq-dev/studio#266](https://github.com/moq-dev/studio/pull/266). It failed
once in each of two agents' full `just shots` runs on 2026-11-25 and 26, the
second in [moq-dev/studio#268](https://github.com/moq-dev/studio/pull/268)'s
work. Both times the host was busy. It passed when run alone and on a rerun,
and 8 stress runs did not reproduce it. Neither failure's assertion was
recorded, so start by reproducing it under load (several copies of the full
`just shots` at once, or the test under a CPU hog) and capturing which step
fails.

Places to suspect first: the `more` helper hovers, clicks, and steps a fixed
two frames before asserting on the engine's scene list, which a command
applied on the runtime may not have reached yet; and the scene on program
being replaced before it is removed. If the root cause is in #266's code
rather than the test, the fix lists #266 under `## Caused by`.
