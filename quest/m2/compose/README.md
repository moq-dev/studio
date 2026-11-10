# Viewer-side composition

## Goal

A studio broadcast carries each independently published source as its own
track, plus a layer layout track that says where and when each one shows.
Players composite those sources over the program and mix their audio in, so
a viewer sees the creator's picture while those sources travel, and can be
toggled, as their own tracks. The end goal is moq's own players, `moq play`
and `@moq/watch`, playing a studio broadcast this way from a layer layout
that hang specifies.

This README's own work is the end-to-end test: a studio broadcast with an
independently published camera and microphone plays in `moq play` and
`@moq/watch` with the camera placed and timed as the studio's preview shows
it, within a stated tolerance, and with its sound mixed in.

## Plan

Replanned 2026-11-08 (maintainer, /quest-plan). This supersedes the
2026-11-07 deferral on
[moq-dev/studio#97](https://github.com/moq-dev/studio/pull/97), which waited
for Published independently to
land before replanning:

- **Reference picture:** ✅ the program with each independently published
  source layered over it (rejected: the full scene rebuilt from every
  source's track, which reverses the 2026-11-04 encoder-cost decision; both,
  chosen per scene).
- **Players:** ✅ native first (rejected: a browser player only; a browser
  page layering sources without the studio's placement rules). The end goal
  is moq's `moq play` and `@moq/watch`.
- **Compositor:** ✅ hang's draft specifies the layer layout, and each player
  implements it with its own code. Parity comes from a shared fixture within a
  tolerance, not shared code. Rejected: moving moq-scene into moq, which
  reverses the 2026-11-02 rule that moq never depends on this repository.
  This replaces the 2026-10-26 "one compositor, three hosts" rule.
- **Prototype:** ✅ a throwaway `moq-studio --watch` on moq-scene with a
  provisional layer format, so the format settles without moq round trips
  (rejected: straight to `moq play`). moq's spec quest starts from it.
- **Audio:** ✅ players mix independently published audio sources with the
  program's, at the gain and mute the layout carries (rejected: video only).
  Since 2026-11-09 the layout carries pan and delay too; see
  [layout](/quest/m2/compose/layout.md).
- **Controls:** ✅ viewer toggles and priorities land in the native player
  (rejected: browser only). Since 2026-11-09 that includes `moq play`; see
  [controls](/quest/m2/compose/controls.md).
- **Browser composition** (moq-scene's browser build layered on
  `@moq/watch`): ✅ deleted, since `@moq/watch` layering supersedes it.
  Server composition stays, replanned to flatten program and layers for
  destinations that cannot compose.
- **Milestone:** ✅ m2 (rejected: m1; staying in m3). The moq quests join
  moq's m3.

Still settled from 2026-10-26: local sources are tracks in the studio's one
broadcast, and guests stay their own broadcasts, named through the hang
catalog's `include` references.

moq's side, in its own quest tree (m3): hang specifies a layer layout track,
and `moq play` and `@moq/watch` composite it.

