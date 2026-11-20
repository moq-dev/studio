# [S] Recordings carry markers as chapters

## Goal

A recording carries the session's markers that fall inside it as chapters,
each titled by the marker's name or number, which a player such as mpv or
VLC lists and seeks to.

## Plan

Planned 2026-11-09 (maintainer, /quest-plan), and held for later: ✅ m2
(rejected: m1). Markers, their list and strip on REVIEW, and a clip around
one shipped in [moq-dev/studio#209](https://github.com/moq-dev/studio/pull/209);
the chapters wait on moq-mux, since the studio cannot place a moment in a
finished file: where the movie starts on the program clock is known only
inside moq-mux's export, and on moq's `main` its `Finish` is opaque.

- Chapters are written when a recording becomes a regular MP4: as it stops
  or the studio quits (`finish` in `rs/moq-studio/src/record.rs`), and as a
  launch repairs one a crash left (`rs/moq-studio/src/record/repair.rs`).
- A marker's moment is its `at` (`rs/moq-studio/src/markers.rs`) on the
  program clock (`Program::clock`'s `capture` in `engine.rs`), which every
  recording shares; moq-mux places it in the movie.
- Recommended: each marker's program-clock reading goes in the session's
  markers file, and each recording's `.unfinished` sidecar names that file,
  so a repair at the next launch finds the markers of the session that
  recorded it. The recorder's own `Marker` type, that sidecar, becomes
  `Sidecar` then, since the two meet in `record.rs`.
- Recommended: a chapter starts at each marker, with none before the first,
  since a marker is a moment, not a span.

## Required

- [moq-mux's MP4 chapters pinned](/quest/m2/pin-mp4-chapters.md) - brings moq-mux's chapters on a finished MP4 into the `moq` submodule
