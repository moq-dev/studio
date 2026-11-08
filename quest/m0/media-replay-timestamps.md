# [S] A replayed media file never repeats a timestamp

## Goal

A media source that starts over while on program publishes its pictures with
strictly increasing timestamps every time, and
`publish::tests::a_media_file_is_published_on_one_timeline`
(`rs/moq-scene/src/publish.rs`) passes on every run.

## Plan

Planned 2026-11-07 (maintainer, /quest-complete): ✅ m0 (rejected: m1), since
the test is in `just test`, so it flakes the merge gate for every PR, and the
repeat may reach viewers. ✅ A quest of its own (rejected: fixing it in the moq
pin bump, [moq-dev/studio#104](https://github.com/moq-dev/studio/pull/104)).

The test fails about half its runs with two equal timestamps in a row: 4 of 8
on `main` at d42fd70, 3 of 6 after the bump to moq 079a646. It came with
[moq-dev/studio#44](https://github.com/moq-dev/studio/pull/44); name that PR
under `## Caused by` if the bug is in its code. The test runs on a paused
clock, so the variation comes from work outside it, such as the encoder's
thread, racing the restart (`feed.play()`). Reproduce it first, then find
whether the published track can repeat a picture's time at a restart (a real
bug) or only the test reads too early, and fix that root cause rather than
retry or loosen the assertion.
