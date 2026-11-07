# [S] A media file's sound plays with its pictures

## Goal

A media file source's sound plays in step with its own pictures in every
output (program, published scenes, recordings, and Listen), instead of about
45 ms behind them.

## Plan

From [moq-dev/studio#81](https://github.com/moq-dev/studio/pull/81), picked
2026-11-07 (maintainer away under /afk, recommended option), m0 by the rule
that fixes join m0.

Pictures show when they are due, but a media source writes its sound to the
bus when it is due, and the bus mixes it a bus latency (50 ms) later.
Broadcast sources already write a bus latency ahead (`playout::Writer`);
media sources should do the same. Test that a media file's picture change
and its sound onset leave an output within one bus block of each other,
measured on the media clock rather than with sleeps.
