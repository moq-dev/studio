# [S] Audio source tracks

## Goal

Each microphone and system-audio source in a `moq-studio` scene publishes its
own capture as an Opus track in the studio's broadcast, listed in that
source's catalog under the studio catalog's `dev.moq.studio` section, beside
the program. Like the picture sources, it encodes while it is in the
collection, whatever the subscriber demand, and uploads only while something
subscribes to it.

## Plan

Cameras, displays, windows, images, and colors already publish this way
(`rs/moq-scene/src/publish.rs`): each source's renditions sit in a nested hang
catalog keyed by source name, so the root `video` and `audio` sections stay
the program's. An audio source adds an `audio` rendition to its own nested
catalog the same way.

Blocked upstream: moq-audio's capture stream is crate-private, and the only
public captures feed either the program bus (`bus::Capture`) or an encoder
(`encode::publish_capture`), never both. The
[pin](/quest/m0/pin-audio-capture.md) brings a capture Producer/Consumer split
that feeds both from one device open, rather than opening each device twice.
Publish the capture after the source's filters but before its gain and mute
(decided 2026-11-02, maintainer): viewers hear filtered audio, as video bakes
in its ML effects, and a composing viewer mixes the levels itself.

Encode like every first-party publisher (decided 2026-10-28, from
[#7101](https://github.com/moq-dev/moq/issues/7101)): an unwatched source
still encodes, so a late viewer or a replay
finds its recent past, but costs no upload.

✅ 2026-11-01 (maintainer, on
[moq-dev/moq#7554](https://github.com/moq-dev/moq/pull/7554)): sources stay
always-on. Rejected: sources on demand, which would make replaying a source
need an armed local subscriber again.
Track names stay stable for a source's life, and an edited source that
changes what it shows gets a new name.

## Required

- [One audio capture feeding the bus and an encoder is pinned](/quest/m0/pin-audio-capture.md) - one device open feeds the mix and the source's own track
