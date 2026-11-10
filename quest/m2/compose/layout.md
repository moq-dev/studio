# [M] Layer layout track

## Goal

A studio broadcast that publishes a source independently also carries a layer
layout track. For each such source, it says where and when that source shows
over the program: its transform, crop, z-order, opacity, and visibility, its
item's keyframes and when its scene went on program, all on the media clock,
and for audio its gain, mute, pan, and delay. Until hang specifies it, the track is the
provisional `dev.moq.studio.layers` in the studio catalog.

## Plan

Replanned 2026-11-08 (maintainer, /quest-plan) around the program with
independent layers over it, rather than the full scene state; see the
[epic](/quest/m2/compose/README.md).

A JSON track in snapshot mode, so a late joiner gets the current state.
Version it, and have a player refuse an unknown version loudly and play the
program alone. Keep it free of local paths and credentials: layers name
tracks and catalog `include` references, never files.

Decided 2026-11-08 (maintainer, on
[moq-dev/studio#117](https://github.com/moq-dev/studio/pull/117)): a
published source stays hidden everywhere the studio composites until this
track lands. Drawing published sources as layers over the program in the
studio's preview is this quest's work, and so is whether recordings include
those layers.

Layers draw over the program in the layout's order, so the studio's preview
of a scene with an independent source shows what viewers see. A layer carries
only what every player can reproduce: transform, crop, z-order, opacity,
visibility, and keyframes. A picture source's track is its capture before any
filter (decided 2026-11-02, maintainer). So an item that applies anything
more, such as a video filter, mask, LUT, blend mode, or background removal,
needs that effect baked into its track, or the option refused for it. Settle
which here. Publishing a source independently refuses one with its
background removed for now, since H.264 carries no transparency, so lifting
that refusal is this track's call too. An audio source's track already carries its sound
after its filters (decided 2026-11-02), so its layer carries what its strip
applies after them: gain, mute, pan, and delay.

Decided 2026-11-09 (maintainer, /quest-audit): ✅ an audio layer carries pan
and delay as well as gain and mute (rejected: stating that a player drops
them). Without the delay, a delayed microphone published independently plays
out of sync with its camera. moq's
[layer spec](https://github.com/moq-dev/moq/blob/main/quest/m3/layers/spec.md) names only gain and mute today,
and starts from the format the prototype settles.

Items carry their keyframes (`moq_scene::Keyframe`), which play from when
their scene went on program. The layout says when that was, so a late joiner
lands at the same point.

Each layer also carries its priority and whether a viewer may toggle it, for
[Viewer controls](/quest/m2/compose/controls.md).

The [watch prototype](/quest/m2/compose/watch.md) is this track's first
reader. What the two settle becomes moq's hang spec draft.
