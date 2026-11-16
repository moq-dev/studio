# [S] Pictures on REVIEW's recording and clip cards

## Goal

Each finished recording's and clip's card on the REVIEW page of
[`mock/review.html`](/quest/m0/ux/mock/review.html) shows the file's first
picture, where it shows a placeholder tile today.

## Plan

Decided 2026-11-15 (agent's recommendation on moq-dev/studio#177, which the
maintainer kept when picking it to merge): ✅ decode through moq-mux's regular MP4 import
(rejected: reading the first sample here with `mp4-atom`, since demuxing
belongs in moq; dropping the pictures from the mock-up).

The picture is the file's first video sample, imported through moq-mux and
decoded small once per file in the shelf's background listing
(`rs/moq-studio/src/shelf.rs`), as the strip decodes keyframes
(`rs/moq-studio/src/past.rs`). A recording under way already shows the live
picture. The placeholder is painted in `rs/moq-studio/src/ui/review.rs`.

## Required

- [MP4 import pinned](/quest/m3/pin-mp4-import.md) - brings moq-mux's regular MP4 import into the `moq` submodule
