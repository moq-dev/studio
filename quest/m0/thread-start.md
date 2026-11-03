# [XS] Worker threads fail loud at startup

## Goal

A studio worker thread (program, audio, source, autosave) that cannot start,
such as a failed thread spawn or Tokio runtime build, reports that error
instead of logging it and leaving the feature dead. A worker the studio needs
to run fails the studio's start; a source, which starts and restarts while
the studio runs, shows the error as that source's failure in the Sources
panel.

## Plan

`engine::clocked` builds each worker's runtime inside the spawned thread and
only logs a failure, and a failed source thread spawn is logged and dropped,
so the caller never learns of either. Build the runtime before spawning, or
hand the result back, and propagate it to whoever starts the worker. Raised on
[#4](https://github.com/moq-dev/studio/pull/4).
