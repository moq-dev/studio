# m1: after the studio ships

## Goal

The work held until m0 ships. A studio broadcast can be composed by its
viewers and by a server from the sources' own tracks and a layout track,
matching the program the studio composites, so a viewer can toggle and
prioritize elements. The studio gains features m0 left out: overlay assets
and restarts, global hotkeys, and isolated guest recordings.

## Plan

Moved out of m0 on 2026-11-04 (maintainer): the program is how a studio
broadcast reaches viewers, and sources are not published to the relay by
default (decided 2026-11-03 on
[moq-dev/studio#21](https://github.com/moq-dev/studio/pull/21)), since
encoding every source for viewers costs more encoder sessions than a consumer
GPU has. Viewer-side composition waits until m0 ships.

Broadened 2026-11-04 (maintainer, /quest-plan): m1 also holds studio
features planned as follow-ups of m0 work but not needed to ship it, rather
than opening an m2 for them.

## Required

- [Viewer-side composition](/quest/m1/compose/README.md) - sources as their own tracks plus a layout track, composed by viewers and by a server
- [Overlays restart on air](/quest/m1/overlay-restart.md) - an overlay's CSS timeline restarts at each cut unless set to keep running
- [Overlay assets](/quest/m1/overlay-assets.md) - overlays use fonts and images from data: URLs or files beside the scene
- [Hotkeys and editor preferences](/quest/m1/hotkeys.md) - system-wide hotkeys, arrow nudge, aspect lock, and persisted editor settings
- [Scene graph view](/quest/m1/scene-graph.md) - a node view of picture and audio scenes, their routing, volume, pan, and waveforms
