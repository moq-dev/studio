# m1: after the studio ships

## Goal

The work held until m0 ships. The studio gains features m0 left out:
overlay assets and restarts, and isolated guest recordings.

## Plan

Moved out of m0 on 2026-11-04 (maintainer): the program is how a studio
broadcast reaches viewers, and sources are not published to the relay by
default (decided 2026-11-03 on
[moq-dev/studio#21](https://github.com/moq-dev/studio/pull/21)), since
encoding every source for viewers costs more encoder sessions than a consumer
GPU has. Viewer-side composition waits until m0 ships.

Viewer-side composition moved to m2 on 2026-11-07 (maintainer, on
[moq-dev/studio#97](https://github.com/moq-dev/studio/pull/97)): no viewer
can fetch a source's track until a source is published independently, which
is m2.

Broadened 2026-11-04 (maintainer, /quest-plan): m1 also holds studio
features planned as follow-ups of m0 work but not needed to ship it, rather
than opening an m2 for them.
