# [S] Audio filters

## Goal

Each audio source's filter chain (gain, noise suppression, noise gate,
compressor, limiter, ducking, and VST3 and CLAP plugins) is configured from
the studio and saved with the scene collection. Ducking lowers one source
(music) when another (the microphone) is active. A plugin that fails or keeps
missing its deadline shows as failed on its source.

## Plan

Changed 2026-11-02 (maintainer), when the studio moved out of moq: the chain
itself, its filters, and the out-of-process plugin host are moq-audio work in
[moq-audio filter chain](https://github.com/moq-dev/moq/blob/main/quest/m3/audio-filters.md).
This quest is the studio's side: the panel, the scene-file shape, and wiring
each source's chain into the program bus.

The scene file names filters and their settings, never a plugin's absolute
path, so a collection stays portable; a missing plugin loads as failed rather
than refusing the file.

## Required

- [moq-audio's filter chain is pinned](/quest/m0/pin-audio-filters.md) - the chain this configures
