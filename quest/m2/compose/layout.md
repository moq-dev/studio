# [M] Guests in the layer layout

## Goal

A guest's broadcast source can be published independently, like a local
source: the program leaves it out, and the provisional
`live.kixel.studio.layers` layout track names the guest's broadcast so a player
places it over the program, and mixes its sound, at the right time.

## Plan

The layout track itself landed for local sources: `rs/moq-scene/src/layout.rs`
and the README's "Layer layout". A guest stays its own broadcast (settled
2026-10-26), so its layer names that broadcast rather than an entry under the
catalog's `live.kixel.studio` `sources`, and carries what a player needs to put
the guest's frames on the program's media clock. Today
`Source::publishes` refuses a broadcast source, since it has no track in the
studio's broadcast.

Bump the layout's `version` if the change is not additive.

The `moq-studio --watch` prototype (`rs/moq-studio/src/watch.rs`) plays only
local sources' layers today: its layers play tracks of the program's own
broadcast on the program's clock, so a guest's layer needs the prototype to
follow the named broadcast, on the offset the layout carries.

## Waiting

- Maintainer: decide how the layout names a guest's broadcast. The epic
  settled on 2026-10-26 that guests stay their own broadcasts, named through
  hang's catalog references, then `contributions`, now `include`
  ([moq-dev/moq#7943](https://github.com/moq-dev/moq/pull/7943)). The hang
  draft at the pin requires an include to share the listing catalog's PTS
  timeline, `media.clock`, and `media.join.interval`, and a guest's broadcast
  keeps its own, so a player that takes `include` at its word places a
  guest's frames at the wrong time. Options:
  - The layout names each guest's broadcast itself and carries the offset
    from the guest's timeline onto the program's media clock, the one the
    studio applies when it plays the guest (recommended): `include` keeps
    its meaning, and the timing stays in the track that already says when
    each layer shows.
  - moq relaxes `include`, or adds a sibling map, for broadcasts on their own
    timeline, with the offset in the catalog: a moq quest and a pin, and every
    hang consumer learns a second kind of include.
  - The studio republishes each guest's tracks in its own broadcast, restamped
    onto the program's clock: no spec change, but it reverses the settled
    "guests stay their own broadcasts" and relays every guest twice.

  Check: the maintainer answers in /quest-unblock, and the choice is recorded
  in this Plan and the epic's.
