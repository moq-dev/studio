# m1: viewers compose

## Goal

A studio broadcast can be composed by its viewers and by a server from the
sources' own tracks and a layout track, matching the program the studio
composites, so a viewer can toggle and prioritize elements.

## Plan

Moved out of m0 on 2026-11-04 (maintainer): the program is how a studio
broadcast reaches viewers, and sources are not published to the relay by
default (decided 2026-11-03 on
[moq-dev/studio#21](https://github.com/moq-dev/studio/pull/21)), since
encoding every source for viewers costs more encoder sessions than a consumer
GPU has. Viewer-side composition waits until m0 ships.

## Required

- [Viewer-side composition](/quest/m1/compose/README.md) - sources as their own tracks plus a layout track, composed by viewers and by a server
