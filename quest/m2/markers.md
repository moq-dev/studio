# [M] Markers and clips

## Goal

A key or a button drops a marker on the show as it runs. The REVIEW page
shows markers on its strip and in a list, a clip is cut around one in a
click, and a recording carries its markers as chapters.

## Plan

Planned 2026-11-09 (maintainer, /quest-plan), and held for later: ✅ m2
(rejected: m1).

- A marker is a time on the program clock and an optional name. Markers of
  a session are kept beside the recordings, not in the scene file.
- A clip around a marker uses Save clip's copy without re-encoding, so the
  same keyframe limits apply.
- Chapters are written when a recording becomes a regular MP4.
- Marker is a hotkey action.

## Required

- [The REVIEW page](/quest/m0/ux/review.md) - the strip and the list markers appear on
