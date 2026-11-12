# [M] The composition is in its broadcast's hang catalog

## Goal

The composition the studio publishes (`rs/moq-scene/src/composition.rs`, the
README's "Composition") travels in the composition broadcast's hang catalog,
each update applied at its timestamp on the program's clock, in place of the
interim `live.kixel.studio.composition` JSON snapshot track. `moq-studio
--watch` (`rs/moq-studio/src/watch.rs`) reads it from that catalog, and
`README.md`'s "Composition" and "Watching" say so.

## Plan

Decided 2026-11-12 (maintainer, /quest-complete on
[moq-dev/studio#146](https://github.com/moq-dev/studio/pull/146)), which
landed the composition on the interim track:

- **Home:** ✅ the composition broadcast's hang catalog, applied at each
  update's timestamp (rejected: shipping the interim JSON snapshot track as
  the format). moq plans timed catalogs in
  [catalog-at-timestamp](https://github.com/moq-dev/moq/blob/main/quest/m2/catalog-at-timestamp.md)
  (m2, on lite-07's time addressing), and the rest of its composition plan in
  [composed broadcasts](https://github.com/moq-dev/moq/blob/main/quest/m3/layers/README.md).
- **Shape:** ✅ kept as #146 has it: each child entry carries `sound`,
  `priority`, and `toggle`, and placements live in `pictures`, each naming a
  child, so one broadcast can show in several places (rejected: one
  placement per child, as moq's spec quest said; moq's
  `quest/m3/layers/spec.md` is updated to match).

Recommended, not decided: the composition goes in a section of the catalog
under the studio's own name until hang specifies one, with `version` kept,
since moq's spec may still change it; the catalog also lists the program's
renditions, as moq's plan has a composition list its main's, so a player
that knows no compositions plays the program from it. A child's offset and
each placement then change with one timed catalog update, and the watcher
applies each when the program plays the frame stamped alike, as it applies
the track's groups now.

## Required

- [hang's timed catalog is pinned](/quest/m2/compose/pin-timed-catalog.md) - hands the studio each catalog update with its timestamp
