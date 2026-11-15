# [S] The media restart test holds under load

## Goal

`source::media_tests::a_media_file_restarts_going_on_program` passes however
busy the host is.

## Plan

It hung once in 60 runs of `source::media_tests::` beside `stress-ng --cpu 96`
on a 32-CPU host, while the other eleven media tests passed every run
(found by moq-dev/studio#181, which fixed the looping test's flake under the
same load). A hang under a paused clock means the test holds the clock, with
`until`'s zero sleep, short of a moment the source sleeps until.

Reproduce it first as #181 did: the test binary run 200 times, 16 at a time,
beside `stress-ng`, each under `timeout 120`. Then find which wait holds. One lead: the comment before the
first `until_time` says waiting for both clips "while the clock holds" starts
them together, but `shown` moves the clock to the intro's start with
`until_time`, so the still clip can begin later, and the fixed 1.1 s may not
cover both plays' ends, which include sound running past the last picture.
Read each play's end from `Feed::run().ended` instead of a fixed offset, as
#181 does for the loop.
