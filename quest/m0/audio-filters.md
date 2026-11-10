# [S] Audio filters

## Goal

Each microphone's and system audio source's filter chain (noise suppression,
noise gate, compressor, limiter, ducking, and VST3 and CLAP plugins) is
configured from the studio and saved with the scene collection. Ducking
lowers one source (music) when another (the microphone) is active. A plugin
that fails or keeps missing its deadline shows as failed on its source.

Guest, broadcast, media, browser, and replay sound stays unfiltered.

## Plan

Changed 2026-11-02 (maintainer), when the studio moved out of moq: the chain
itself, its filters, and the out-of-process plugin host are moq-audio work in
[moq-audio filter chain](https://github.com/moq-dev/moq/blob/main/quest/m3/audio-filters.md),
which moves to moq's m3 in [moq-dev/moq#8421](https://github.com/moq-dev/moq/pull/8421).
This quest is the studio's side: the panel, the scene-file shape, and wiring
each source's chain into the program bus.

Each microphone's and system audio's own track encodes its capture, before
the bus applies gain and mute (`Driver::listen` in
`rs/moq-scene/src/source.rs`). The chain goes before that tap, so the track
carries filtered sound (decided 2026-11-02, maintainer), unlike a picture's
track, which is its capture before any filter.

The scene file names filters and their settings, never a plugin's absolute
path, so a collection stays portable; a missing plugin loads as failed rather
than refusing the file.

Decided 2026-11-09 (maintainer, /quest-audit): ✅ the Goal covers
microphones and system audio only (rejected: widening moq's chain to bus
channels). moq's chain runs on the capture producer, so only captured sources
have one; sound that reaches the bus another way never passes through it.

## Required

- [moq-audio's filter chain is pinned](/quest/m0/pin-audio-filters.md) - the chain this configures
