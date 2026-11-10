# [M] A small live picture of every scene and source

## Goal

moq-scene draws a thumbnail of any scene or source on request: a small
texture on the compositor's device, refreshed a few times a second, and only
while something shows it. The studio's Scenes panel shows one beside each
scene. The cost of a strip of them is measured.

## Plan

The scene pads, the Add page's cards, and the multiview all show live
pictures of things that are not on air (see
[the LIVE frame](/quest/m0/ux/mock/shell-live.webp)). Today only the program
and one preview are drawn (`Compositor` in
`rs/moq-scene/src/compositor.rs`), and a scene nobody watches costs nothing.
Keep that: a thumbnail nobody shows is not drawn, as the preview is skipped
when no tab shows it.

- A thumbnail is a scene at its own shape, scaled down, drawn at a low rate
  on the program clock. A source's is its latest frame through its own crop
  and filters.
- A scene that is on air is not drawn twice: its thumbnail samples the
  program.
- A capture that is not otherwise in use is not opened for a thumbnail. A
  source with no frame yet shows nothing, and the UI draws its icon instead.
  Live pictures of devices that are not sources yet are
  [the capture listing's](/quest/m0/pin-capture-list.md).
- Thumbnails never reach the program, a published scene, or a recording.

Measure, per AGENTS.md: frame time and GPU copies with zero, six, and thirty
thumbnails shown, and that the program's render time does not move. Settle the
size and rate from that.

It does not wait on the [toolkit spike](/quest/m0/ux/spike.md): the texture is
toolkit-free, and today's Scenes panel is its first consumer.
