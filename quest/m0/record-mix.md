# [S] A recording picks its mix

## Goal

A recording in `moq-studio` picks any of the collection's mixes, so it can
keep what the stream leaves out, such as the creator's music, while the
broadcast carries the mix on program. A recording on the same mix as the
program records as it does today.

## Plan

Split from [Audio mixes](https://github.com/moq-dev/studio/pull/32), which
landed the mixes, routing, meters, delay, listening, and the program's pick
(`[mixes] program`).

A recording reads the program from a broadcast the studio keeps off the relay
(`Recording` in `rs/moq-studio/src/engine.rs`, from
[moq-dev/studio#21](https://github.com/moq-dev/studio/pull/21)), beside the
recording's own canvas-size encoder: its Opus track is a second encoding of
the mix on program, made only while a recording reads it. Point that encoding
at the mix the recording picks instead, so a mix the creator left off the
stream is never fetchable from the relay.
The pick lives in the scene file beside `[mixes] program`, and the Output
panel shows it. Update `README.md`.

## Related

- [Destinations](/quest/m0/destinations.md) - each destination picks a mix the same way
