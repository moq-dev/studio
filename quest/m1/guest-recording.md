# [M] Isolated guest recordings

## Goal

Recording each source also writes a file for each guest and other broadcast
source, so an editor gets every guest's camera and microphone on its own.
The tracks are copied as received, with no re-encode.

## Plan

From [moq-dev/studio#18](https://github.com/moq-dev/studio/pull/18).

Decided 2026-11-04 with the maintainer (/quest-plan):

- ✅ **Extend the existing record-each-source option**, rather than adding a
  separate guest toggle: one switch gives an editor every isolated track.

Implementation:

- `rs/moq-studio/src/record.rs` records each source's own track from the
  studio's local broadcast. A guest's tracks come from the relay origin the
  studio subscribes through, so read them from there and name the file after
  the guest.
- A guest that leaves and rejoins mid-recording continues in a new file, or
  in the same one if its timeline continues; pick the one moq-mux supports
  and say which in `README.md`.
- Test with a guest published to a local relay: its file holds the guest's
  samples unchanged.
