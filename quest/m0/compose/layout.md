# [M] Layout track

## Goal

A studio broadcast carries a layout track: moq-scene's live scene state
(items, the tracks they show, transforms, transitions, and each element's
priority and whether a viewer may toggle it) as a hang JSON track on the
media clock. Any host that reads it composites the same frame the studio
shows. The hang catalog and `drafts/draft-lcurley-moq-hang.md` specify it.

## Plan

Serialize moq-scene's own model rather than a second layout schema, so the
studio, server, and browser cannot drift. Version it and refuse an unknown
version loudly. Keep it free of local paths and credentials: items name
tracks and contribution references, never files. A viewer that refuses a
layout version plays the program instead and says why. A late joiner needs the
current state, so use the JSON track's snapshot mode.

Decided 2026-11-02 (maintainer, follow-up planning after
[#6](https://github.com/moq-dev/studio/pull/6)): clones and filters travel in
the layout, with each item naming the source track it shows and the filters
applied to it. Only a filter that cannot render the same way in every viewer
is baked into a track of its own. The picture source tracks are enough to
start; audio source tracks join the layout when they land.

## Related

- [Source tracks](/quest/m0/compose/sources.md) - the audio source tracks a layout also names once they land
