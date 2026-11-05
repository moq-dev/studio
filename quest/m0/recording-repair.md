# [S] Recover crashed recordings

## Goal

When the studio starts and finds a recording that a crash or power loss left
unfinished, it repairs the file without asking, writes the result as
`<name>.repaired.mp4` beside the original, and reports the repair in Health
and the log. The original stays untouched.

## Plan

From [moq-dev/studio#18](https://github.com/moq-dev/studio/pull/18).

Decided 2026-11-04 with the maintainer (/quest-plan):

- ✅ **Repair automatically and keep the original.** The repaired copy is
  written to a new file, so a repair that goes wrong never costs the only
  copy, and no prompt blocks startup.
  Rejected: repair in place (risks the only copy), ask first (a prompt at
  every launch after a crash).

Implementation:

- `moq_mux::container::mp4::repair` rebuilds a file from its fragments.
- The recording directory and file naming are in `rs/moq-studio/src/record.rs`.
  Mark a file as in progress with a sidecar (for example `<name>.mp4.part`)
  written when it starts and deleted when it finishes. Launch repairs each
  file with a sidecar, then deletes the sidecar once `<name>.repaired.mp4` is
  written, so the same file is never repaired twice and the original keeps
  its name.
- Repair each source's file as well as the program's.
- A test writes a recording, stops before finishing it, and checks that
  launch produces a playable `.repaired.mp4` holding the samples written.
