# [XS] moq-mux's regular MP4 import is pinned

## Goal

The `moq` submodule includes moq-mux's import of a regular, non-fragmented
MP4, from
[moq-mux imports a regular MP4](https://github.com/moq-dev/moq/blob/main/quest/m3/mp4-import.md).

## Plan

Once the wait clears, advance it with `just upgrade-moq` in its own PR,
fixing whatever the new API breaks here.

Here in m3 because the upstream quest is in moq's m3 (maintainer, 2026-11-05);
move both up together if the studio needs it sooner.

## Waiting

- moq-dev/moq: the upstream quest above lands. Check: that quest's PR has
  merged on moq's `main`, deleting its file, and `ContainerFormat` in
  `rs/moq-mux/src/import/format.rs` offers regular MP4. A quest file that
  moved, or went away without its work landing, does not count.
