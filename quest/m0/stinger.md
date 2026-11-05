# [S] Stinger transitions

## Goal

A scene change in `moq-studio` can run a stinger: a video clip played over the
program that switches scenes at its cut point, as broadcast graphics do.

## Plan

Split from the keyframes quest on 2026-11-04 (maintainer). A
stinger is a transition kind beside fade, slide, wipe, and move
([moq-dev/studio#33](https://github.com/moq-dev/studio/pull/33)), drawn on the
program clock, whose clip is a media file source. It must start its clip as
the transition starts, or its cut point lands at random. Its settings (clip
and cut point) change through `Edit`, so they undo.

## Required

- [Media file source](/quest/m0/media-file.md) - a clip that starts on demand
