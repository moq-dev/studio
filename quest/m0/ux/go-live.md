# [L] Going live from inside the app

## Goal

`moq-studio` starts with no arguments and no terminal, and a broadcaster goes
live and stops from the window, as in
[`mock/golive.html`](/quest/m0/ux/mock/golive.html):

- **Opened with no show**, it lists recent shows with a picture of each, a
  New show card, and Open a show file.
- **Off air**, the frame around the picture is neutral, the tag reads OFF
  AIR, and the live control is GO LIVE.
- **GO LIVE** opens a short sheet: where the stream is going, each
  destination with a toggle and its state, and a checklist that fills in
  (camera, a microphone level that moves, an encoder that keeps up, a
  connection that carries the bitrate), then one confirm. A failed line is
  one plain sentence and its fix.
- **On air**, the control is the clock, and END STREAM under the pointer,
  held to confirm. Ending delivers the broadcast's tail before the session
  closes, as quitting does today.
- **The health dot** opens every destination, the viewers, and the encoder,
  and its details: the worst problem first, in plain words with its cause,
  which is today's Health panel.

Where the stream goes is set in Settings: the relay's address, its token, a
failover relay, the broadcast's name, and whether to encode only while
someone watches. Go live and End stream are hotkey actions.

## Plan

Today the studio is live from launch when `--connect` is given and never
otherwise, and the relay, token, broadcast name, failover, on-demand, and
recordings folder are flags (`rs/moq-studio/src/main.rs`). A packaged app is
opened by a double click, so none of that can stay a flag only.

- ✅ **Connecting is a runtime action.** The engine opens and closes its
  relay sessions on Go live and End stream. Composing, previewing, and
  recording never needed a relay and still do not.
- **Flags stay** as overrides for scripts and tests: a flag wins over the
  saved setting for that run and is never written back.
- **Where it is saved:** the relay address, broadcast name, and options are
  per show, beside the scene file, not in it, since the scene file is meant
  to be shared and names no relay. Tokens go in the operating system's
  credential store, as [destinations](/quest/m0/destinations.md) decided for
  stream keys, and never in a file.
- **The checklist is guided setup's probes** (`rs/moq-studio/src/setup.rs`):
  devices, the encoder's pace, and the uplink. The uplink probe runs against
  the relay about to be used and is skipped with its reason when a
  connection reports no estimate.
- Recommended, not decided: a failed check never blocks going live. Its
  sentence and its fix are advice, since a streamer on a bad connection may
  still choose to go. Only having nothing to send, no destination switched
  on, disables the confirm.
- GO LIVE is not red. Red stays the on-air state alone.
- **A replay of the program** needs the relay's cache, so off air it says so
  rather than failing.
- **Recent shows** are kept per user. A show's picture is the last frame of
  its program, saved on quit.
- Guests' room flags move with the rest; the room key stays a file the
  setting points at.

[Destinations](/quest/m0/destinations.md) adds RTMP and WHIP rows to the same
sheet and the same health popover; this quest lands them for MoQ relays
alone.

Human check: a real stream to a real relay with the maintainer's token,
ended and started again; the credential store's own prompt on each desktop;
and opening the app from a file manager with no terminal.

## Required

- [One live picture](/quest/m0/ux/live.md) - the live control, the health dot, and the frame this drives

## Related

- [Destinations](/quest/m0/destinations.md) - RTMP and WHIP rows in this sheet
- [Packaging](/quest/m0/packaging/README.md) - an app opened by a double click has no flags
