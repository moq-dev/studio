# [S] Clap-to-measure sync delay

## Goal

A clap in front of a camera, seen and heard, measures how far a microphone
leads that camera, and sets the microphone's `delay` to it, instead of the
creator guessing the number.

## Plan

Split from [Audio mixes](https://github.com/moq-dev/studio/pull/32), which
landed the per-source `delay` the measurement sets.

Measure both onsets on the program clock: the clap's sound in the
microphone's audio before it is delayed, and the frame where the hands meet in
the camera's pictures. The difference, bounded by moq-audio's
`bus::MAX_DELAY`, becomes the delay, as one undoable edit. A clap that is not
found, or found in only one of the two, fails with the reason rather than
setting a guess.

The studio sees a source's audio only through its strip's level meter, read
once a frame, which is too coarse to time a clap. Timing it needs the
source's samples, or an onset, with their position on the bus clock, which is
moq-audio work: land it there first and pin it.
