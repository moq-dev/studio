# [M] Viewer controls

## Goal

A viewer watching a studio broadcast can switch off any layer the layout
marks toggleable (a facecam, a screen share), and stops fetching its track.
The layout's per-layer priorities steer delivery, so under congestion the
least important sources degrade first. The creator sets both per source in
the studio.

## Plan

Decided 2026-11-08 (maintainer, /quest-plan): the controls land in the native
player, the [watch prototype](/quest/m2/compose/watch.md) (rejected: browser
only). moq's players take them from hang's layer spec.

Decided 2026-11-09 (maintainer, /quest-audit): ✅ viewer toggles also land in
moq's native player,
[moq play composites layers](https://github.com/moq-dev/moq/blob/main/quest/m3/layers/play.md)
(rejected: leaving them to the prototype alone, which
[hang-layers](/quest/m2/compose/hang-layers.md) deletes, so the native
player's toggles would have no home). moq adds the toggles to that quest in
its own tree.

Map layer priority onto the subscription priorities moq-lite already has
(`moq/doc/concept/moq-lite.md`), rather than a second mechanism. A toggled-off
layer is unsubscribed, so it costs the viewer and the relay nothing. The
studio keeps encoding it, so toggling it back on can reach its recent past.
Subscriptions are per track, not per layer: a source shown by several layers
stays subscribed while any of them is on, at the highest priority among them.

## Required

- [moq-studio watch prototype](/quest/m2/compose/watch.md) - the viewer that exposes the toggles
