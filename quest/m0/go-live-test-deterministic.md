# [S] The one-click Go live test passes under any host load

## Goal

`go_live_from_no_relay_and_end_on_confirm` in `rs/moq-studio/src/shots.rs` passes on a loaded host as reliably as on an idle one.
It still proves the window's flow: GO LIVE with no relay asks for one, Go live now saves it and goes live once the check passes, and End stream asks before ending.
The go-live check itself does not change, and a test still exercises the real check against a real relay somewhere.

## Plan

Decided 2026-11-23 (maintainer, /quest-run follow-up from [moq-dev/studio#237](https://github.com/moq-dev/studio/pull/237)): plan it, in m0, since `just shots` runs nightly and in every UI PR's validation, where a flake costs a rerun and a misread.

Caused by [moq-dev/studio#240](https://github.com/moq-dev/studio/pull/240), which added the test.
It failed once during a full `just shots` run under heavy host load while #237 merged main, and passed alone three times and in two later full runs.
The test drives a real software encoder and a real relay, and waits up to `SETTLE` for the uplink check (seconds of measurement) to pass, asserting "Go live anyway" never shows; on a loaded host the measurement can fall short and the check fails.

Recommended, for the implementer to confirm: reproduce it first under load (for example with a parallel CPU hog), then have the flow test take the check's outcome from a seam it already has or a small one (a check result fed in, or the clock paused so the measurement's window is not wall time), rather than raising `SETTLE` or loosening the assertion.
Leave the real measurement to a test that asserts on the measurement alone, if one does not already exist.

