# [L] Live encode control

## Goal

`moq-studio` adapts its encode live without a restart: the program and each
source publish simulcast rungs that follow the relay's bandwidth grant, a rung
nobody watches stops encoding, and bitrate and resolution change mid-stream
through catalog updates. OBS's multitrack output does
part of this only on Windows with specific GPUs; this works on every
platform.

## Plan

Today the studio publishes one H.264 rendition at fixed settings. Its tracks
claim their bitrate on the connection's `bandwidth::Allocator`, but nothing
follows the grant. Reuse what exists rather than adding a studio
path: the grant follower in `moq_video::encode` and `moq_mux::rate`, the
catalog's in-place updates and new renditions (`doc/concept/hang.md`). The
encode producer already cuts a keyframe when a subscriber joins
(`encode/cuts.rs`), so each rung inherits that and can run a long GOP. The
Health panel's track table already shows each track's bitrate and reporting
viewers; add each rung's grant beside them, which needs moq to expose a
track's share of the allocator.

## Related

- [Simulcast rung disable](https://github.com/moq-dev/moq/blob/main/quest/m1/simulcast-rung-disable.md) - grant-driven rung disable this builds on
- [moq-audio follows the grant](https://github.com/moq-dev/moq/blob/main/quest/m2/2848-follow-the-bandwidth-grant-in-moq-audio-instead-of.md) - the audio half
- [Scene editing](/quest/m0/scene-editing.md) - canvas resize through a new rendition
