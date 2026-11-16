# [M] Settings

## Goal

Everything a broadcaster sets once is in one Settings window, opened from the
title bar's gear, as in
[`mock/settings.html`](/quest/m0/ux/mock/settings.html):

- **Show:** landscape or portrait, the canvas size and frame rate as a few
  named choices with the numbers behind Custom, and per-scene sizes behind a
  disclosure.
- **Stream:** the relay, its token, the failover, the broadcast's name, and
  on demand, as today's Stream tab holds them in `<show>.stream.toml`.
- **Recording:** the folder, what a recording holds, and each source too.
- **Hotkeys:** today's Hotkeys panel, on keycaps.
- **Guests:** the room, its key, the join page, and how long a link lasts.
- **Advanced:** publishing a scene as its own broadcast, publishing a source
  independently, the virtual camera, the scene graph, the effects level.

It replaces the Output and Hotkeys panels, the View menu, and the flags that
have no other home. The File and Edit menus go too: reload, undo, redo, copy,
and paste keep their keys and are reachable from the show's menu in the title
bar.

## Plan

- The mock-up lists a Sound pane. Its one setting, the output Listen plays
  on, is on the MIX page instead, so there is none.
  The Guests pane has no frame; it follows the Stream pane's shape.
- Each setting keeps its home on disk: the scene file for what belongs to
  the show (canvas, scene sizes, published scenes), per-user settings for
  what belongs to the operator (hotkeys, snapping, effects, the listen
  output), and the per-show file from go live for where it streams.
- A setting with a cost says it in one line where it is set: an encoder
  session per published scene, the bitrate a canvas needs, a source leaving
  the program when published independently.
- Encoder sessions, encode times, and the broadcast path, which the Output
  panel shows today, are the health dot's details, not settings.
- A change that restarts an encoder or a track, such as a new canvas size,
  says so before it applies while on air.
- The scene graph (`rs/moq-studio/src/ui/graph.rs`) opens from Advanced in a
  window of its own.
- The Recording pane's folder and the Guests pane's room, key, join page, and
  link lifetime are already fields of `<show>.stream.toml` (`recordings`,
  `room`, `room_key`, `join_page`, `guest_expiry`), which go live
  ([moq-dev/studio#175](https://github.com/moq-dev/studio/pull/175)) saves
  but gives no UI; the Recording tab shows and opens the folder but cannot
  change it. These panes edit them there, and a flag that overrides one
  shows it as the flag's, as the Stream pane does.
- A room or relay change reaches the open show's guests at once: the guest
  room renews its links against the new room and relay, instead of only after
  the show is reopened, as go live left it.

Human check: whether each setting is where a streamer would look for it, and
the words on each pane.
