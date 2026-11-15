# [S] The looping media test holds under load

## Goal

`source::media_tests::a_looping_media_file_starts_over` passes however busy
the host is.

## Plan

Seen once in a full `just test` on a shared host (moq-dev/studio#172):
the picture shown at 950 ms on the paused clock was the 700 ms frame, not the
900 ms one (`rs/moq-scene/src/source.rs`, the `Duration::from_millis(900)`
assertion). Four reruns of the media tests alone and one full run passed.
The test pauses tokio's clock, which auto-advances whenever the runtime is
idle, while the file decodes on a thread of its own, so under load the clock
can run past frames the decoder has not delivered yet. Reproduce it under CPU
load first, such as with `stress-ng` beside the test, then make the test wait
on the decoder rather than on the clock, or have the clock wait for the
decoder in the source itself if a real run can show the same skip.
