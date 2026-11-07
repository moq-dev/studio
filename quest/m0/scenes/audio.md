# [M] Audio scenes

## Goal

The audio mixes become audio-only scenes. An audio scene holds sounds
(sources' audio, with gain, pan, mute, and delay) and other audio scenes, so a
"Mics" scene can feed both "Stream" and "Headphones". **On program** picks the
audio scene the broadcast carries; each published scene and each recording
picks one. Audio stays independent of cuts.

## Plan

Decided 2026-11-05 with the maintainer (/quest-plan); see the
[epic](/quest/m0/scenes/README.md).

- Replaces `[mixes]` and `[audio.<source>]` in the scene file with no
  migration; nothing is released.
- An audio scene cannot contain itself, directly or through others.
- The Audio panel stays a mixer: a strip per sound with fader, pan, meter,
  and a send to each audio scene. Listen and On program work as today.
- The guest return feed (a mix-minus) and the sync delay keep working; a
  guest hears the audio scene on program without their own sound.
- Switching or editing an audio scene never drops or repeats audio, as with
  mixes today.
- `README.md` documents audio scenes.
