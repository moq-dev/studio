# [M] Server composition

## Goal

A headless compositor binary in this repository subscribes to a studio broadcast's sources and
layout track and publishes the composited program, headless (a GPU, or
lavapipe without one). It can push that program to RTMP and WHIP through the
studio's egress library, so a hosted service makes the fallback composite and
the creator saves the upload.

## Plan

Decided 2026-10-26 (maintainer): its own command, not a mode of
`moq transcode`. A hosted service embeds the same library. It publishes its
own broadcast, never the studio's names: each publisher produces its own
stream.

Changed 2026-11-02 (maintainer): it was planned as `moq compose` in
moq-cli, but moq-scene moved here with the studio, so the command lives here
too and moq never depends on this repository. Settle its name in this quest
and document it in `README.md`.

## Required

- [Layout track](/quest/m1/compose/layout.md) - what the server composes
