# [S] Viewer priorities

## Goal

The `moq-studio --watch` prototype fetches each source of a composition at the
priority the composition gives it, so under congestion the least important
sources degrade first, as the creator ranked them in the studio.

## Plan

Decided 2026-11-08 (maintainer, /quest-plan): the controls land in the native
player, the `moq-studio --watch` prototype (`rs/moq-studio/src/watch.rs`;
rejected: browser only). moq's players take them from hang's layer spec.

Decided 2026-11-09 (maintainer, /quest-audit): ✅ viewer toggles also land in
moq's native player,
[moq play composites layers](https://github.com/moq-dev/moq/blob/main/quest/m3/layers/play.md)
(rejected: leaving them to the prototype alone, which
[hang-layers](/quest/m2/compose/hang-layers.md) deletes, so the native
player's toggles would have no home). moq adds the toggles to that quest in
its own tree.

[moq-dev/studio#218](https://github.com/moq-dev/studio/pull/218) landed the
rest of this quest: the creator sets each published source's priority and
whether viewers may switch it off (`[published.<name>]` in the scene file, and
the inspector), the composition carries both on each source's entry, and
`--watch` leaves out a source the viewer switched off, unsubscribing it.

Map priority onto the subscription priorities moq-lite already has
(`moq/doc/concept/moq-lite.md`), rather than a second mechanism. A source's
`priority` is relative to the program's tracks: each of its tracks is fetched
at hang's priority for its kind plus it, saturating
(`Child::priority` in `rs/moq-scene/src/composition.rs`). Both decode
consumers' `Options` take the subscription priority on moq's `main` since
[moq-dev/moq#9282](https://github.com/moq-dev/moq/pull/9282); once the pin is
past it, carry the priority on `Source::Layer`, and pass it to the
layer's video and audio decode consumers (`follow_video` and `follow_audio` in
`rs/moq-scene/src/source.rs`). A composition that changes only a layer's
priority keeps its feed (`Sources::sync` reuses one that `plays_alike`), so
move the priority in place with each consumer's `set_priority`, as the feed
already moves its offset. Prove it with a layer test that reads the priority
off the publisher's aggregate subscription, before and after a change.

## Required

- [The studio catches up to moq's main](/quest/m0/pin-moq-tip.md) - moves the pin past moq-dev/moq#9282 and absorbs the tip's other breaks
