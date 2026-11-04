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

A recording reads the studio's broadcast the way a viewer does
(`rs/moq-studio/src/record.rs`), so a mix other than the program's needs an
Opus track of its own. Keep that track off the relay: a mix the creator left
off the stream must not be fetchable from it, as source tracks stay local
after [Live encode control](/quest/m0/encode-control.md). Encode it only while
a recording reads it, and record the program's video beside it in one file.
The pick lives in the scene file beside `[mixes] program`, and the Output
panel shows it. Update `README.md`.

## Required

- [Live encode control](/quest/m0/encode-control.md) - the local broadcast a mix track stays off the relay in

## Related

- [Destinations](/quest/m0/destinations.md) - each destination picks a mix the same way
