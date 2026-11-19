# [XS] Live with a camera that keeps out of the way

## Goal

The maintainer has streamed a few times with a facecam set to **Keep out of
the way** over real games and slides, and it moves when it should and no
more often. If it does not, the timings or weights it chooses by change until
it does.

## Plan

The analysis, the choice, and the glide are tested in moq-scene on synthetic
reads and on the GPU, and BUILD's `build_keep_clear` view shows what it saw.
How often it moves on real content is a judgment no test makes. The knobs are
constants in `rs/moq-scene/src/place.rs`: `SETTLE` (1 s before a choice),
`WORST` (10 s in the worst place before it moves anyway), `BETTER` (how much
clearer a place must be), `APART` and `CHANGING` (what counts as the content
changing kind), `WEIGHT` (motion 0.5, text 0.8, HUD 1.0), and `MIDDLE` (a
corner's lead over an edge middle).

## Waiting

- Maintainer: put a camera over a game and over a slide deck with **Keep out
  of the way** on (inspector, Place), stream or record a few sessions, and
  note each move that was wrong, late, or too frequent. An agent cannot judge
  how a move reads to a viewer or which part of a real game matters; the
  tests already check the frames and the rules. Done when it moves as
  intended, or the change it needs is written into this Plan.
