# [M] moq-studio watch prototype

## Goal

`moq-studio --watch <url>` plays a studio broadcast: the program, with each
source the layer layout names composited over it on the media clock, and
their audio mixed with the program's. A broadcast without a layer layout plays
its program alone.

## Plan

Decided 2026-11-08 (maintainer, /quest-plan): a throwaway prototype, so the
layer format settles without moq round trips before hang specifies it
(rejected: going straight to `moq play`). It lives in `moq-studio`, not its
own binary, since [hang-layers](/quest/m2/compose/hang-layers.md) deletes it
once `moq play` plays layers.

Build it on moq-scene, which already plays remote broadcasts as sources for
guests (`rs/moq-studio/src/guests.rs`): a scene that the layout drives rather
than a scene file. `README.md` documents the flag while it exists.

Its working format is the starting point for moq's hang layer spec, which
waits on this quest.

## Required

- [Layer layout track](/quest/m2/compose/layout.md) - what the prototype plays
