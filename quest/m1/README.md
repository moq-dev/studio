# m1: after the studio ships

## Goal

The work held until m0 ships: what the redesigned studio gains that no
streaming app has today, and the shortest way in for someone leaving OBS.
Each builds on [Broadcaster UX](/quest/m0/ux/README.md).

## Plan

Viewer-side composition moved out of m0 on 2026-11-04 (maintainer): the program is how a studio
broadcast reaches viewers, and sources are not published to the relay by
default (decided 2026-11-03 on
[moq-dev/studio#21](https://github.com/moq-dev/studio/pull/21)), since
encoding every source for viewers costs more encoder sessions than a consumer
GPU has. Viewer-side composition waits until m0 ships.

Viewer-side composition moved to m2 on 2026-11-07 (maintainer, on
[moq-dev/studio#97](https://github.com/moq-dev/studio/pull/97)): no viewer
can fetch a source's track until a source is published independently, which
was m2 then and shipped in
[moq-dev/studio#117](https://github.com/moq-dev/studio/pull/117).

Broadened 2026-11-04 (maintainer, /quest-plan): m1 also holds studio
features planned as follow-ups of m0 work but not needed to ship it, rather
than opening an m2 for them.

Planned 2026-11-09 (maintainer, /quest-plan): redesigning what the studio
already does is m0, and the features below, which do not exist yet, are here
(rejected: all in m0; all in m1). Three more were held for
[m2](/quest/m2/README.md): a phone as a remote, markers and clips, and a
command palette. Dropped, not planned: a rundown that fills overlays,
one-click looks and a brand kit, active-speaker layouts, control surfaces,
and a rehearsal mode.

## Required

- [A camera that keeps out of the way](/quest/m1/auto-place.md) - a layer moves off what matters in the picture, decided on the machine
- [Scenes that lay themselves out](/quest/m1/auto-layout.md) - slots and roles that reflow when a guest joins
- [A capture that went black says so](/quest/m1/capture-health.md) - a badge and the fix on the layer
- [A mini controller for one monitor](/quest/m1/mini-controller.md) - an always-on-top bar and a tray item, never on stream
- [The studio hides itself and private apps from capture](/quest/m1/privacy-shield.md) - its own windows and marked apps left out of display capture
- [A source that follows the game](/quest/m1/follow-game.md) - whichever app is in front, without picking again
- [Drop anything on the picture](/quest/m1/drop.md) - a dragged file or a paste becomes a layer
- [Opening an OBS scene collection](/quest/m1/obs-import.md) - a show made from an OBS collection, with a report
- [A multiview window](/quest/m1/multiview.md) - every scene and source on a second monitor, with tally
- [Private panels](/quest/m1/panels.md) - chat and dashboards as windows that are never on stream
- [Landscape and portrait at once](/quest/m1/dual-format.md) - both formats from one show
- [moq-video's foreground app is pinned](/quest/m1/pin-capture-foreground.md) - which app is in front, for following the game
- [moq-video's capture exclusion is pinned](/quest/m1/pin-capture-exclude.md) - leaving chosen apps out of a display capture
