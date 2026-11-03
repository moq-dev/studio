# [S] Audio sources report stalls

## Goal

A live audio source (a microphone or system audio) that stops delivering
samples reads "stalled" in the Sources panel and Health, as a live camera does
after a second without a picture.

## Plan

Audio capture runs inside the audio bus today, which reports no last-delivery
time per source, so the stall check video sources have cannot see it. The
capture Producer/Consumer split that the audio-capture pin brings gives the
studio its own handle on each capture; read delivery from there rather than
adding a side channel in the bus. Raised on
[#5](https://github.com/moq-dev/studio/pull/5).

## Required

- [moq-audio's capture split is pinned](/quest/m0/pin-audio-capture.md) - the
  per-capture handle this reads
