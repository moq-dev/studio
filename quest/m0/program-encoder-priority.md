# [S] The program's encoder before the sources'

## Goal

Going live gets the program its hardware encode sessions even when the
sources on program took every one the GPU allows while off air. Today the
program opens its encoders at GO LIVE, after the sources' replay encoders
(`rs/moq-scene/src/publish.rs`, which encode a source shown on program so a
replay has its recent past) already hold theirs, so on a GPU with a session
cap, such as a GeForce card's, a show with many sources on program goes live
on the CPU encoder or not at all. Before going live from the window, the
program opened first, at launch.

## Plan

Raised by the review of
[moq-dev/studio#175](https://github.com/moq-dev/studio/pull/175). The go-live
sheet's Encoder check already opens the same encoder with the sources'
sessions held, so the failure shows before the confirm; this makes it not
happen. `Sessions` counts encoders but cannot know a cap, which a driver
reports only by refusing an open.

Decided by the maintainer: when the program's encoder cannot open, release
the replay encoders of the sources on program, open the program's, then let
each source reopen as sessions allow, its replay starting over (rejected:
opening the program's encoders when the show opens and holding them idle off
air, at one or two sessions and their memory for the whole session; leaving
it to the go-live sheet's Encoder check alone). A replay is a convenience and
the program is what viewers see; the cost falls only on a machine at its cap.
