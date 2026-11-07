# [S] Unpublishing a scene ends its broadcast cleanly

## Goal

When a scene is unpublished, renamed, or removed, its viewers receive the
end of each track as a clean finish, never an `Unroutable` error, and
`a_published_scene_has_its_own_broadcast` passes every time.

## Plan

From [moq-dev/studio#81](https://github.com/moq-dev/studio/pull/81), picked
2026-11-07 (maintainer away under /afk, recommended option), m0 by the rule
that fixes join m0.

The studio closes a scene's broadcast as soon as its encoders finish their
tracks locally, so the relay can unroute it before a track's end reaches the
viewer. The test above failed 3 times in about 33 runs on a loaded host from
this race. Quitting already waits until the relay has what it was sent
(`Output::close` in `rs/moq-studio/src/engine.rs`).

- ✅ **Unpublishing waits the same way** before it closes the broadcast.
  Rejected for now: changing moq-net to deliver a finished track's end after
  its broadcast closes, which would land upstream first and is only worth it
  if the studio's wait cannot cover every case.
