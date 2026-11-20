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
(`Child::priority` in `rs/moq-scene/src/composition.rs`). Once the wait
clears, advance the gitlink with `just upgrade-moq` in this PR unless a bump
already has, carry the priority on `Source::Layer`, and pass it to the
layer's video and audio decode consumers (`follow_video` and `follow_audio` in
`rs/moq-scene/src/source.rs`). Prove it with a layer test that reads the
priority off the publisher's aggregate subscription.

## Waiting

- moq-dev/moq: `moq_video::decode::Options` and `moq_audio::decode::Options`
  take the subscription priority, as
  [Decode consumers take a subscription priority](https://github.com/moq-dev/moq/blob/main/quest/m1/decode-priority.md)
  plans (being planned in [moq-dev/moq#9247](https://github.com/moq-dev/moq/pull/9247)), which each consumer fixes today at hang's
  `PRIORITY.video` and `PRIORITY.audio`. Check: both options carry a priority
  on moq's `main`.
