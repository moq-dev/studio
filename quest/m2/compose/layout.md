# [L] The studio publishes a composition

## Goal

The studio publishes a composition: a broadcast of its own whose catalog
lists other broadcasts, the program and each independently published source,
local or guest, with where and when each shows and how its sound mixes. Each
listed broadcast keeps its own timeline, and the composition carries the
offset that puts it on the composition's media clock. The `moq-studio --watch`
prototype (`rs/moq-studio/src/watch.rs`) plays a composition, guests
included, and it replaces the provisional `live.kixel.studio.layers` track
(`rs/moq-scene/src/layout.rs`, and the README's "Layer layout").

## Plan

Decided 2026-11-12 (maintainer, /quest-unblock), settling how the layout
names a guest:

- **Naming:** ✅ a composition, a catalog of catalogs (rejected: the layer
  layout naming each guest's broadcast with an offset; moq relaxing hang's
  `include` for broadcasts on their own timeline; republishing guests'
  tracks in the studio's broadcast). hang's `include`
  ([moq-dev/moq#7943](https://github.com/moq-dev/moq/pull/7943)) requires an
  included broadcast to share the listing catalog's PTS timeline,
  `media.clock`, and `media.join.interval`, which a guest's does not; a
  composition keeps `include`'s meaning and says how each timeline maps.
- **Home:** ✅ its own broadcast (rejected: a section of the program's
  catalog, which is `include` with a different clock). The program is one of
  its children, and a player that knows no compositions still plays the
  program.
- **Local sources:** ✅ every independently published source is its own
  broadcast (rejected: layers naming a broadcast and track, with local
  sources staying tracks of the program's broadcast at offset zero). This
  reverses the 2026-10-26 rule that local sources are tracks in the studio's
  one broadcast, so the composition lists only catalogs.
- **Format:** ✅ the studio prototypes it, provisional and versioned, before
  moq's hang specifies it (rejected: straight to hang's spec), as the
  2026-11-08 layer layout did.

Recommended, not decided: the composition's media clock is the program's, so
the program's offset is zero; each child's entry carries what the provisional
layer format carries per layer today (placement, crop, opacity, visibility,
keyframes, toggleability, priority, and gain, mute, pan, and delay for
sound); and a change is a new snapshot group stamped on the composition's
clock, as the layers track is now.

`Source::publishes` refuses a broadcast source today, since it has no track
in the studio's broadcast; under a composition a guest needs no track of the
studio's. `README.md`'s "Published independently" and "Layer layout" become
the composition's.

Bump the format's `version`, since naming broadcasts instead of tracks is not
additive.
