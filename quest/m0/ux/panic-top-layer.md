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
microphones, the guests, and the items together. Whichever option is picked,
the panic key's tooltip (`rs/moq-studio/src/ui/shell.rs`) and its README
entry say what stays on, and the engine test covers it.

## Waiting

- Maintainer: should the panic key also take the top layer off program?
  Options: (a) hide every visible top-layer item in the panic edit; (b) hide
  only top-layer items showing a live capture (camera, display, window,
  browser, broadcast), keeping images, colors, and text such as a logo;
  (c) leave the top layer on, as now, and only document it. Recommendation:
  (a), since panic exists for what is said or shown on air, a safe scene
  usually carries its own branding, and a rule about which sources count as
  live is one more thing to get wrong under pressure. An agent cannot pick
  what a broadcaster expects to stay on screen.
