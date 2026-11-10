# [M] A source that follows the game

## Goal

Add offers a source that shows whichever game or app is in front, rather
than one fixed window. Nothing needs picking again when the game restarts,
opens a new window, or the streamer switches to another. The studio itself
and [private apps](/quest/m1/privacy-shield.md) are never followed: the
source holds its last picture, or a card the show chooses, while one of them
is in front.

## Plan

OBS users ask for this by name, and get it from a plugin or another app.

- The foreground app and its window come from moq-video, through
  [its pin](/quest/m1/pin-capture-foreground.md). The source is a new kind
  in moq-scene beside `window`.
- It can be narrowed: any app, only full-screen ones, or a list.
- A switch between apps is a cut inside the source, keeping the layer's
  place and fitting the new window by the layer's fit rule.
- Where the desktop does not report the foreground app, the card says so and
  cannot be added.

Human check: real games, a restart mid-stream, and alt-tabbing fast.

## Required

- [Adding a source by picking a picture](/quest/m0/ux/add.md) - the card that adds it
- [moq-video's foreground app is pinned](/quest/m1/pin-capture-foreground.md) - which app and window are in front
