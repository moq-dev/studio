# [S] The panic key and the top layer

## Goal

Pressing the panic key leaves on air only what the maintainer intends: today
it mutes every microphone and guest and cuts to the safe scene, but the top
layer, drawn over program through every cut, stays on. A camera, display
capture, or browser source placed there, such as a facecam shown on every
scene, is still on air after panic.

## Plan

Raised by Pullfrog's review of
[moq-dev/studio#171](https://github.com/moq-dev/studio/pull/171#pullrequestreview-5910298509).
`State::panic` (`rs/moq-studio/src/engine.rs`) makes one undoable edit and a
cut; hiding top-layer items would join that edit (`Draft::set_item` with
`visible: false` on each `Layer::top()` item), so one undo brings back the
microphones, the guests, and the items together. The panic key's tooltip
(`rs/moq-studio/src/ui/shell.rs`) and its README entry (which today say the
top layer stays on) say what it hides, and the engine test covers it.

Decided by the maintainer
([comment](https://github.com/moq-dev/studio/pull/171#issuecomment-6898166608),
2026-11-15): the panic edit hides every visible top-layer item, whatever it
shows. Rejected: hiding only live captures and keeping a logo (a rule about
which sources count as live is one more thing to get wrong under pressure);
leaving the top layer on and documenting it. A safe scene usually carries its
own branding.
