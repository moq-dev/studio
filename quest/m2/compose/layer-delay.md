# [S] A lost group stalls a layer no longer than its offset

## Goal

A layer's pictures wait in their subscription only as long as that layer
plays behind, its own offset plus the viewer's playout, so a lost group
stalls a guest close to live for that long, not for the fixed `LAYER_DELAY`
(two full playouts) every layer holds today.

## Plan

Decided 2026-11-20 (maintainer, /quest-plan): ✅ the studio sets each layer's
`Delay::max` from its own composition offset (rejected: a moq-net subscription
that skips by a reader's deadline apart from `Delay::max`, which is the same
thing once the max matches how far behind the reader plays; leaving it).

`LAYER_DELAY` in `rs/moq-scene/src/source.rs` bounds every layer's video and
sound (`follow_video` and `follow_audio`), since its pictures wait undecoded
in the subscription. Its doc counts two playouts, the studio's of the layer's
broadcast, which the offset carries, and the viewer's of the program; keep
the viewer's in the per-layer max. A layer's offset changes with each
composition. Sound moves its max in place with
`moq_audio::decode::Consumer::set_max_delay`, as `moq play` does; moq_video's
decode consumer has no such setter, so recommended: resubscribe a layer's
video when its offset moves more than a frame from the one its max was set
for, shrinking as well as growing, rather than on every change. Alternatives: add a `set_max_delay` to moq_video's decode
consumer in moq first, mirroring moq_audio's, or have the layer read pictures
out at once into a queue of its own and keep the subscription's max small;
pick whichever measures better on a lossy link.

Found by [moq-dev/studio#146](https://github.com/moq-dev/studio/pull/146).
