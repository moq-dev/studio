# [S] The cue test passes every time on CI

## Goal

`engine::tests::a_cue_waits_for_take` passes on every run of nightly's
Software render job, with its root cause fixed rather than its assertion
loosened or retried.

## Plan

It failed twice in a row on that job, then passed on the same commit
([run 44848291984](https://github.com/moq-dev/studio/actions/runs/44848291984),
jobs 136673346473 and 136697397976 failed, 136713241257 passed), and it passes
locally (`just render-sw`'s filter on lavapipe). It fails at
`rs/moq-studio/src/engine.rs:4449`: `refused(Action::Mute("host"))` says
"`host` has no sound", so `controls::sounds` found no strip for the `host`
broadcast source, which every opened broadcast source has
(`Source::has_audio`). So `state.sources` did not hold `host` at that moment
on a slow runner: find what opens and closes a source no scene shows, and when,
and make the test wait on that event, or the engine hold it, whichever is the
intended behavior. The assertion landed with
[moq-dev/studio#219](https://github.com/moq-dev/studio/pull/219).
