# [M] A clip around an older marker, from the recording

## Goal

A marker older than the last 30 seconds still gets a clip around it, in one
click on REVIEW, cut from the recording that holds it without re-encoding,
as a clip from the recent past is. A marker no recording holds says so.

## Plan

A clip from the recent past copies the program's track, which keeps only 30
seconds (`rs/moq-studio/src/replay.rs`), so today a marker past that has no
clip. A recording holds the rest: import it through moq-mux, from the
keyframe before the marker's span, and export the span as Save clip does, so
the same keyframe limits apply. A finished recording is a regular MP4, which
moq-mux's regular MP4 import reads; one still being written is a fragmented
MP4, which its fMP4 import already reads.

Here in m3 because the import it needs is pinned in m3.

## Required

- [MP4 import pinned](/quest/m3/pin-mp4-import.md) - brings moq-mux's regular MP4 import into the `moq` submodule
