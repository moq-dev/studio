# [S] A replay's sound plays with its pictures

## Goal

A replay source's sound plays in step with its own pictures in every output
(program, published scenes, recordings, and Listen), instead of a bus
latency (50 ms) behind them. This is about replays at normal speed; a slowed
replay stays silent, as it is today.

## Plan

From [moq-dev/studio#93](https://github.com/moq-dev/studio/pull/93), picked
2026-11-07 (maintainer away under /afk, recommended option), m0 by the rule
that fixes join m0.

#93 moved media file sources to write their sound a bus latency ahead
through `playout::Writer`, with their pictures handed over at the same lead
and shown when due. Replay sources still write their sound when it is due,
so the bus mixes it 50 ms late. Apply the same fix (a replay has no loop to
handle) and test it the way #93's `a_media_files_sound_plays_with_its_pictures`
does.
