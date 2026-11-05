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

Clones and video filters travel in the layout, not as tracks: a picture
source's track is its capture before any filter, and a clone publishes none,
so the layout names the source a clone shows and the video filters each
source and item applies (decided 2026-11-02, maintainer), with crops and
blend modes. Only a video filter that cannot render the same way in every
viewer is baked into a track of its own. A mask names a picture source's
track, and a LUT a `lut` source, whose `.cube` table publishes no track yet,
so a viewer needs one carrying it, as an image's file is. Audio is the other
way round: an audio source's track carries its sound after its audio filters
and before its gain and mute (decided 2026-11-02, maintainer), so
the layout carries only what is applied after that point, such as gain and
mute, and never names an audio filter for a viewer to apply again.

A browser source publishes no track of its own (decided 2026-11-03,
maintainer, on [moq-dev/studio#8](https://github.com/moq-dev/studio/pull/8)).
Each has a "viewers load this page" option, off by default and refused for a
localhost URL: on, the layout carries the page's URL and viewers load it
themselves; off, viewers see the page only in the program. A replay source
(`rs/moq-scene/src/replay.rs`) shows a span of another source's past, so the
layout says when it is hidden and when its span has ended, rather than leaving
a viewer on its last frame.

Only sources the creator publishes independently
([Published independently](/quest/m0/publish-independently.md)) have tracks a
viewer can fetch; the rest reach viewers only in the program.
