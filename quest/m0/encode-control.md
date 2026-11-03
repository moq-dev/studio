# [S] Keyframes on request

## Goal

Each rung of the program's and every source's simulcast ladder cuts a keyframe
when a viewer joins past its newest group, so the studio can run a longer GOP
than its 2 s default at no cost to how fast a viewer starts.

## Plan

The ladder (`rs/moq-scene/src/ladder.rs`) encodes through
`moq_video::encode::Encoder` and publishes through
`moq_video::encode::Producer`, whose join cuts are private to moq's own
`publish_capture` (`encode/cuts.rs`, `Producer::wanted`): the studio cannot see
whether a join asks for a group past the newest. This needs moq to expose that
on `encode::Producer` (or a cut-on-join helper beside it), then a moq pin bump.
With it, advertise `track::Info::refresh` on each rung's track, cut in
`Rendition::encode` when a join asks, and measure the bitrate a longer GOP saves
before picking one.

## Related

- [Simulcast rung disable](https://github.com/moq-dev/moq/blob/main/quest/m1/simulcast-rung-disable.md) - the grant-driven rung disable the ladder follows
- [moq-audio follows the grant](https://github.com/moq-dev/moq/blob/main/quest/m2/2848-follow-the-bandwidth-grant-in-moq-audio-instead-of.md) - the audio half, which still only reserves
- [Scene editing](/quest/m0/scene-editing.md) - canvas resize, which has to rebuild the program's ladder
