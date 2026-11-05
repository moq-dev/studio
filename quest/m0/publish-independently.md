# [S] Published independently

## Goal

A source in `moq-studio` has a "Published independently" option, off by
default. Ticked, the source's own track is published to the relay for viewers
and services to fetch, and the source is left out of the composited program,
so a service can place a webcam or overlay itself. Unticked, the source
reaches viewers only through the program.

## Plan

Decided 2026-11-03 (maintainer, on
[moq-dev/studio#21](https://github.com/moq-dev/studio/pull/21)): sources are
not published to the relay by default, since every encoding costs an encoder
session and consumer GPUs have few. A source published independently gets one
rendition, as every source track does, while the program keeps its
two sizes. It counts as used, so it encodes for as long as it is published
(decided 2026-11-04, maintainer): a source encodes while it is on program
(shown in a scene on air, or, for a microphone or system audio, played by the
mix on program) or while something reads its track, and naming a published
source among those on program (`Publish::set_on_program` in
`rs/moq-scene/src/publish.rs`) keeps it encoding.

Low priority: this is for services that move webcams and overlays themselves,
and none exists yet. The option lives on the source, in the scene file, so it
travels with the scene. The option and its effect on the program change what a
user does, so `README.md` documents them.

## Related

- [Viewer-side composition](/quest/m1/compose/README.md) - the m1 epic that composes from independently published sources
