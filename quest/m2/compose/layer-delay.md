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

Built in [moq-dev/studio#231](https://github.com/moq-dev/studio/pull/231): each
layer measures the longest wait between a frame arriving and coming due, plus
a 100 ms `LEEWAY` (`VIDEO_DELAY`), and sets its `Delay::max` from that, not
from the offset, which also carries the two publishers' clock difference.
Video resubscribes when the reach moves; sound moves its max in place.
A deterministic test shows a lost group now stalls a layer within its lag, not
`LAYER_DELAY`.

Decided 2026-11-23 (maintainer, via /quest-run), after measuring on a lossy
link (loopback `netem`, two 320x180 30 fps broadcasts with a 1 s GOP, a layer
about 400 ms behind): under 1% loss plus a 600 ms outage every 5 s, the layer
stalled 0.77 of the program's stall with this max, against 0.63 with
`LAYER_DELAY`, and 0.69 with a 500 ms leeway. Random loss of 2% and 5% did
not stall it. An outage's data is retransmitted, not lost, yet a max close to
the lag drops it. Likely cause: a skipped group takes its group of pictures'
on-time tail with it.

- ✅ Hold this PR until moq explains why a short `Delay::max` drops video that
  retransmission would deliver in time (rejected: land as is; land with a
  500 ms `LEEWAY`). Resume once it lands, with the studio's `LEEWAY` and
  resubscribe policy set from what moq changes.
- ✅ No in-place `set_max_delay` on `moq_video::decode::Consumer` for this
  quest (rejected: plan it in moq now). It measured no better on stalls than
  resubscribing; revisit in moq only if decoder reopen CPU shows up once this
  lands.

## Waiting

- moq-dev/moq: the investigation into why a short `Delay::max` drops video
  that retransmission would deliver in time lands. Check: a moq quest named
  for it has merged on moq's `main`, deleting its file, with a moq-side test
  that a group arriving late within the reader's lag is delivered, not
  skipped; then bump the `moq` gitlink here and re-measure the outage runs in
  [moq-dev/studio#231](https://github.com/moq-dev/studio/pull/231). A quest
  file that moved, or went away without its work landing, does not count.
