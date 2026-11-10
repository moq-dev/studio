# [L] Web studio

## Goal

The full studio runs in a browser from a page this repository builds and
serves:
- the same egui UI and moq-scene compositor on WebGPU
- capture through getUserMedia and getDisplayMedia
- encode and decode through WebCodecs
- publishing over WebTransport through moq-net

Browser sources are absent. So are RTMP and WHIP destinations. A page cannot
open raw TCP, and moq-webrtc's WHIP client binds native UDP sockets, so only MoQ
output remains. Each missing feature is hidden, not shown broken.

The page lands in vertical slices: video first (camera and screen sources on
one canvas, published to a relay), then audio (capture, mixing, playback).
The finished page reaches parity with the native app for everything a browser
can reach.

## Plan

Decided 2026-10-22: a full wasm studio, not a remote control for a native
one. Media goes through browser backends in moq-video and moq-audio, not a
bridge to js/publish and js/watch, so frames never cross into a second media
stack.

Changed 2026-11-02 (maintainer), when the studio moved out of moq: the
backends land in moq first with their own moq-side tests, rather than in the
same PR as this page, and the page follows each pin bump. The page was
planned for moq's demo/web; it now ships from this repository, and where it
is hosted is settled in this quest.

Decided 2026-11-04 (maintainer): a page has no local paths, so an image or
media source's file comes from a browser file picker for the session, and
resolving the scene file's paths in the browser is settled here.

Decided 2026-11-09 (maintainer, /quest-audit): ✅ m2, with both pins
(rejected: keep in m0). moq removed all WASM support on 2026-10-31, and its
browser-backend quests
([video](https://github.com/moq-dev/moq/blob/main/quest/m5/video-web.md),
[audio](https://github.com/moq-dev/moq/blob/main/quest/m5/audio-web.md)) are
deferred in its m5 with no other consumer. Nothing in moq's tree plans a
browser WebTransport session for moq-net either, which the page publishes
through: that needs a moq quest and a pin here before this is ready.

## Required

- [moq-video's browser backends are pinned](/quest/m2/pin-browser-video.md) - the video slice's capture, codecs, and GPU import
- [moq-audio's browser backends are pinned](/quest/m2/pin-browser-audio.md) - the audio slice's capture, playback, and codecs
