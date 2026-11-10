# [M] Server composition

## Goal

A headless compositor binary in this repository subscribes to a studio
broadcast's program, its independently published sources, and its layer
layout, and publishes the flattened program, headless (a GPU, or lavapipe
without one). It can push that program to RTMP and WHIP through the studio's
egress library. So destinations that cannot compose, such as RTMP, WHIP, HLS,
and legacy players, still see the sources the creator published
independently.

## Plan

Replanned 2026-11-08 (maintainer, /quest-plan): it flattens the program and
its layers, since a source published independently leaves the program; see
the [epic](/quest/m2/compose/README.md).

Decided 2026-10-26 (maintainer): its own command, not a mode of
`moq transcode`. A hosted service embeds the same library. It publishes its
own broadcast, never the studio's names: each publisher produces its own
stream.

Changed 2026-11-02 (maintainer): it was planned as `moq compose` in
moq-cli, but moq-scene moved here with the studio, so the command lives here
too and moq never depends on this repository. Settle its name in this quest
and document it in `README.md`.

## Required

- [Layer layout track](/quest/m2/compose/layout.md) - what the server flattens
- [Destinations](/quest/m0/destinations.md) - the egress library the server pushes RTMP and WHIP through
