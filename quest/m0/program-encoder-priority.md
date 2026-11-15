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

Recommended: when the program's encoder cannot open, release the replay
encoders of the sources on program, open the program's, then let each source
reopen as sessions allow, its replay starting over. A replay is a convenience
and the program is what viewers see; the cost falls only on a machine at its
cap.

## Waiting

- Maintainer: how the program takes priority. (a) Recommended: release the
  sources' replay encoders when the program's cannot open, as the Plan says.
  (b) Open the program's encoders when the show opens and hold them idle off
  air, as launch did before, at one or two sessions and their memory for the
  whole session. (c) Leave it to the go-live sheet's Encoder check, which
  reports it. An agent cannot pick: each trades replay, memory, or a failed
  go-live on capped GPUs. Check: this item is answered here.
