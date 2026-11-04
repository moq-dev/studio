# [S] Audio source tracks

## Goal

Each microphone and system-audio source in a `moq-studio` scene has its own
capture as an Opus track in the studio's broadcast, listed in that source's
catalog under the studio catalog's `dev.moq.studio` section, beside the
program, as the picture sources already are. An isolated recording and a
replay of an audio source read that track.

## Plan

Cameras, displays, windows, images, and colors already publish this way
(`rs/moq-scene/src/publish.rs`): each source's renditions sit in a nested hang
catalog keyed by source name, so the root `video` and `audio` sections stay
the program's. An audio source adds an `audio` rendition to its own nested
catalog the same way.

moq-audio's capture Producer/Consumer split (`capture::Producer`) feeds both
the bus and an encoder from one device open, rather than opening each device
twice.
Publish the capture after the source's filters but before its gain and mute
(decided 2026-11-02, maintainer): a recording or replay holds filtered audio,
as video bakes in its ML effects, and the levels stay the program's.

Changed 2026-11-03 (maintainer, on
[moq-dev/studio#21](https://github.com/moq-dev/studio/pull/21)): a source's
own track encodes only while something in the studio uses it (an isolated
recording, a replay), and reaches the relay only when the creator publishes
the source independently ([Published independently](/quest/m0/publish-independently.md)).
This replaces the earlier always-on decision (2026-11-01, on
[moq-dev/moq#7554](https://github.com/moq-dev/moq/pull/7554)), because
encoding every source costs encoder sessions a consumer machine does not
have; an audio source follows the picture sources' rule.

Track names stay stable for a source's life, and an edited source that
changes what it shows gets a new name.
