# [S] AAC for destinations

## Goal

A destination can take AAC: the creator, or a preset, picks it for a
destination whose ingest requires it, and the studio encodes it with the
platform's encoder on macOS and Windows. Destinations that share a picture
scene, an audio scene, and encode settings still share one encoder. On Linux,
choosing AAC fails loudly when no encoder is available.

## Plan

Decided 2026-11-09 (maintainer, /quest-audit): ✅ split from
[Destinations](/quest/m0/destinations.md) (rejected: leaving that quest
blocked on AAC), which lands with Opus first.

Decided 2026-10-22 with the maintainer, carried from that quest:

- **AAC for RTMP ingests,** which commonly require H.264 and AAC. Opus stays
  the default.
- **AAC encoders:** platform encoders only, from the AudioToolbox and Media
  Foundation quests. moq-audio has no software AAC tier, and none is
  planned. On Linux, choosing AAC for a destination fails loudly when no
  encoder is available. It never silently falls back to another codec.

## Required

- [Destinations](/quest/m0/destinations.md) - the egress library and the per-destination codec choice AAC joins
- [Platform AAC encoders are pinned](/quest/m0/pin-aac-encode.md) - AAC on macOS and Windows
