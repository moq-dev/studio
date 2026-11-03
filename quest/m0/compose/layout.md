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

Specify the source listings in the hang draft too, then move the studio to
what it settles on. Until then sources are listed provisionally as
`dev.moq.studio.sources.<name>` in the studio catalog, each a nested hang
catalog (decided 2026-11-02, maintainer; `rs/moq-scene/src/publish.rs`).

Clones and filters travel in the layout, not as tracks: a source's track is
its capture before any filter, and a clone publishes none, so the layout names
the source a clone shows and the filters each source and item applies
(decided 2026-11-02, maintainer).

## Required

- [Audio source tracks](/quest/m0/compose/sources.md) - the audio tracks a layout names
