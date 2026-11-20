# [S] A lost group stalls a layer no longer than its offset

## Goal

A layer's pictures wait in their subscription only as long as that layer
plays behind, so a lost group stalls a guest 100 ms behind for about 100 ms,
not the fixed `LAYER_DELAY` (two playouts) every layer holds today.

## Plan

Decided 2026-11-20 (maintainer, /quest-plan): ✅ the studio sets each layer's
`Delay::max` from its own composition offset (rejected: a moq-net subscription
that skips by a reader's deadline apart from `Delay::max`, which is the same
thing once the max matches how far behind the reader plays; leaving it).

`LAYER_DELAY` in `rs/moq-scene/src/source.rs` bounds every layer's pictures,
since they wait undecoded in the subscription. A layer's offset changes with
each composition, and a `Delay` change moves nothing already started, so
recommended: resubscribe a layer's video when its offset outgrows its max by
more than a frame, rather than on every change. Alternatively the layer reads
pictures out at once into a queue of its own, and keeps the subscription's max
small; pick whichever measures better on a lossy link.

Found by [moq-dev/studio#146](https://github.com/moq-dev/studio/pull/146).
