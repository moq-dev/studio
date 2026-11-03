# [XS] Worker threads fail loud at startup

## Goal

A studio worker thread (program, audio, source, autosave) that cannot start,
such as a failed Tokio runtime build, fails the studio's start with that error
instead of logging it and leaving the feature dead.

## Plan

`engine::clocked` builds each worker's runtime inside the spawned thread and
only logs a failure, so the caller never learns of it. Build the runtime
before spawning, or hand the result back, and propagate it to whoever starts
the worker. Raised on [#4](https://github.com/moq-dev/studio/pull/4).
