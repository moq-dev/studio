# [L] Studio destinations

## Goal

A creator adds any number of RTMP, RTMPS, and WHIP destinations alongside
MoQ, each with its own codec and bitrate, and goes live to all of them at
once. Destinations that take the same picture scene and audio scene with identical
encode settings share one encoder. Each destination is a row in the go-live
sheet and in the health dot's list, with its status and errors. Audio is Opus
here; AAC is
[AAC for destinations](/quest/m0/destinations-aac.md).

The fan-out lands here as a UI-free egress library, with the studio as its
first consumer. The library subscribes once to a MoQ broadcast and remuxes it
to any number of destinations, never decoding or encoding. A hosted service
can run the same code server-side.

## Plan

Decided 2026-10-22 with the maintainer:

- **A library, not a gateway:** this repository ships no gateway service and
  no CLI mode for it. It lands in the same PR as its studio consumer, not
  ahead of it.
- **Library behavior:**
  - **Name:** settle the crate's name in this quest with the maintainer.
  - **Destinations:** each one is added and removed at runtime and names the
    renditions it takes. The publisher already produced every codec it
    needs, so there is nothing to transcode.
  - **Isolation:** each destination has its own bounded buffer. A
    destination that accepts a connection and then stops reading drops its
    own media, never blocks the others, and never grows memory. A test
    covers that stalled peer.
  - **Redials:** a dropped destination redials with backoff and can be
    cancelled. A refusal such as a rejected stream key is terminal and is
    reported instead of retried.
  - **RTMPS:** dials `rtmps://` through moq-rtmp, whose TLS route sends no
    `tcUrl` today, which several ingest servers reject. moq's
    [RTMPS keeps `tcUrl`](https://github.com/moq-dev/moq/blob/main/quest/m3/rtmp-rtmps-tcurl.md)
    fixes that, planned in
    [moq-dev/moq#8421](https://github.com/moq-dev/moq/pull/8421).
  - **Untrusted destination URLs:** a server-side host must be able to
    refuse private and loopback addresses, so dialing goes through the
    policy moq's
    [caller-supplied dial policy](https://github.com/moq-dev/moq/blob/main/quest/m3/webrtc-dial-policy.md)
    adds (also planned in #8421), rather than resolving any URL it is handed.
- **The studio chooses codecs, not a server:**
  - Opus by default.
  - E-RTMP HEVC or AV1 only when the creator picks it for a destination that
    accepts it.
- **Stream keys** go in the OS credential store, never in the scene file,
  which is meant to be shared and versioned. Prefer a maintained crate such
  as `keyring`.
- **Benchmarks:**
  - Sweep concurrent broadcasts against destinations per broadcast in the
    library.
  - Sweep distinct encoder-sharing keys (picture scene, audio scene, and settings) against
    destinations per key in the studio. Finding a destination's encoder must
    not scan every encoder group, and identical keys must never spawn a
    duplicate encoder.
- **Defaults:** offer presets (resolution, frame rate, keyframe interval,
  CBR) for common ingest constraints without naming platforms. The creator
  can override every field.

Decided 2026-11-09 (maintainer, /quest-audit):

- ✅ **Unblock it from AAC** (rejected: leave it blocked on AAC). RTMP with
  Opus and WHIP work at today's pin, so AAC, its codec decisions, and the
  wait on moq's platform encoders moved to
  [AAC for destinations](/quest/m0/destinations-aac.md).
- ✅ **The two moq changes are moq's quests** (rejected: planning them inline
  here, with no moq quest or pin). This quest uses them through
  [the pin](/quest/m0/pin-egress-dial.md).

Changed 2026-11-09 (maintainer, /quest-plan): going live and stopping, as
actions and as hotkeys, are
[Going live from inside the app](/quest/m0/ux/go-live.md), which lands them
for MoQ relays. This quest adds its destinations to that sheet rather than
building a second way to start a stream.

## Required

- [Going live from inside the app](/quest/m0/ux/go-live.md) - the go-live sheet, the live control, and the health list its destinations join
- [moq's egress dial changes are pinned](/quest/m0/pin-egress-dial.md) - RTMPS that keeps `tcUrl`, and a dial policy the caller supplies

## Related

- [Server composition](/quest/m2/compose/server.md) - a source published independently leaves the program, so a destination shows it only once the server flattens it
