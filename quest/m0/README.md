# m0: the studio

## Goal

A native, cross-platform streaming studio (`rs/moq-studio`) for creators and
teams who rely on OBS today, plus the same app built to wasm. It composites
cameras, screens, media, browser sources, and remote MoQ guests into scenes
on the GPU with moq-video and moq-audio, across several canvases at once
(landscape and portrait), and goes live to a MoQ relay and directly to RTMP,
RTMPS, and WHIP ingests.

Non-goals: Electron or any webview-hosted UI, a native or wasm plugin API,
and server-side fan-out to third-party ingests (a hosted service may offer
that outside this repository).

This README's own work is an end-to-end test in `rs/moq-studio`: with no
window, a scene goes live to a local relay and a local moq-rtmp listener, and
both receive it.

## Plan

Decided 2026-10-22 with the maintainer (/quest-plan):

- **Home:** first `rs/moq-studio` in moq, beside moq-cli. Changed
  2026-11-02 (below).
- **UI:** egui with egui_dock. It is the only Rust toolkit with all of
  same-device zero-copy wgpu textures, multiple native windows, docking, and
  a proven wasm build. Slint has no docking and an unstable wgpu interop
  feature, gpui has no wgpu on macOS or Windows, and Tauri is a webview. The
  app owns the wgpu device.
- **Compositor:** a UI-free crate, `rs/moq-scene`: scene graph, sources,
  transforms, transitions, and canvases on wgpu. The program audio bus goes
  in moq-audio, whose only mixer today is output-only
  (`rs/moq-audio/src/playback/mixer.rs`). Both grow slice by slice with the
  studio UI that consumes them, never ahead of it.
- **Outputs:** the studio publishes to MoQ and pushes every RTMP(S) and WHIP
  destination itself. It remuxes its own broadcast through a UI-free egress
  library that a hosted service can also run server-side. There is no
  gateway service and no moq-cli dependency, because a studio user does not
  run a server. Destinations that take the same canvas and audio mix with
  identical encode settings share one encoder. The studio picks codecs per destination: Opus by default, AAC
  where an ingest requires it, from platform encoders only.
- **Extensibility:** first-party features plus browser sources (CEF
  offscreen, desktop only). A plugin API is a large surface to maintain with
  no consumer.
- **Web:** the full studio in wasm, served as a web page, not a remote
  control. Its media runs on new browser backends in moq-video and moq-audio
  (getUserMedia, getDisplayMedia, WebCodecs) rather than a bridge to
  js/publish and js/watch, so one Rust media path drives both builds.
- **Guests:** guests and phone cameras join a moq-room prefix the studio
  owns, through a web join link or QR code.
- **Naming:** quests name no streaming platforms; ingest requirements are
  written as generic constraints.
- **Docs:** inline. Each quest extends `README.md`.

Changed 2026-11-02 with the maintainer, moving the studio out of moq into
this repository: the app carries enough non-moq surface (egui, CEF, virtual
cameras, packaging, signing) that one monorepo no longer pays off.

- **Pin:** moq is a `moq` git submodule whose crates are path dependencies,
  as in moq.pro, so the studio compiles against one moq checkout.
- **moq-scene** moved here with the app, its only consumer. The
  [viewer-side composition](/quest/m0/compose/README.md) epic moved whole:
  browser composition ships as a package from this repository layered on
  `@moq/watch`, and the server composite is a headless binary here rather
  than a moq-cli subcommand, so moq never depends on the studio.
- **Egress library:** lives here first, beside its studio consumer, and moves
  to moq only when a hosted service adopts it.
- **Library work** (moq-video and moq-audio browser backends, moq-audio's
  filter chain, platform AAC encoders, capture) stays in moq's quest tree and
  lands there first with its own moq-side test. The studio quest that uses it
  waits on a pin quest here, which `just upgrade-moq` advances. This replaces
  "never ahead of the studio UI that consumes them" above.

Dropped from the first scoping round: relay fan-out ("encode once, the relay
restreams") and its upload savings. Direct push with shared encoders matches
OBS multistream, and the hosted version is product work outside this
repository.

## Required

- [Studio on macOS with capture granted](/quest/m0/app-macos.md) - camera, screen, and audio capture run on a Mac, not only Linux
- [Studio on Windows](/quest/m0/app-windows.md) - the studio runs on Windows, not only Linux
- [Viewer-side composition](/quest/m0/compose/README.md) - sources as their own tracks plus a layout track, composed by viewers, with a server-composed program as the fallback
- [The program encoder drains](/quest/m0/program-drain.md) - a hardware encoder never loses the program's last frames at exit or on pause
- [Scene editing](/quest/m0/scene-editing.md) - rename and reorder scenes, and resize the canvas while live through a new rendition
- [Broadcast sources play in sync](/quest/m0/broadcast-sync.md) - subscribed broadcasts and guests play audio and video on the publisher's timestamps, with `moq play`'s delay
- [Wayland display and window sources](/quest/m0/wayland-sources.md) - portal-picked sources that reopen without the picker
- [Live encode control](/quest/m0/encode-control.md) - simulcast rungs that follow the bandwidth grant, live bitrate and resolution, keyframes on request
- [Health grades source tracks](/quest/m0/health-sources.md) - the Health panel covers each published source track, not only the program
- [Audio sources report stalls](/quest/m0/audio-stall.md) - a dead microphone or system audio reads stalled like a camera
- [Relay failover](/quest/m0/failover.md) - the same epoch on two relays, so a relay failure does not drop the stream
- [Instant replay and DVR](/quest/m0/replay.md) - replays and clips from held groups, and viewer scrubbing through the archive
- [HTML/CSS overlays](/quest/m0/overlays.md) - script-free HTML and CSS graphics through Blitz natively and the DOM in the browser
- [Animation and transitions](/quest/m0/animation.md) - move, slide, wipe, and stinger transitions, and keyframed item properties
- [Video effects](/quest/m0/effects.md) - a curated effect set with CSS meanings, and ML effects baked into their source
- [Snapping and guides](/quest/m0/snapping.md) - preview moves snap to edges, centers, and other items
- [Destinations](/quest/m0/destinations.md) - RTMP, RTMPS, and WHIP push with per-destination codecs and shared encoders, on a UI-free egress library a hosted service can reuse
- [Multiple canvases](/quest/m0/multi-canvas.md) - landscape and portrait canvases composite at once from shared sources
- [Guests](/quest/m0/guests.md) - remote guests and phone cameras join through a moq-room link or QR code and appear as sources
- [Guest return feed](/quest/m0/guest-return.md) - guests see the program and hear a mix-minus
- [Guest token refresh](/quest/m0/guest-refresh.md) - a show outlasts the guest link's expiry without new links
- [Audio mixes](/quest/m0/mixes.md) - per-output mixes, meters, monitoring, and sync offsets
- [Audio filters](/quest/m0/audio-filters.md) - per-source noise suppression, gate, compressor, limiter, and plugins, configured from the studio
- [Recording](/quest/m0/recording.md) - each canvas's program, and optionally each source, recorded to disk alongside live outputs
- [Browser sources](/quest/m0/browser-source.md) - web overlays rendered by out-of-process CEF into the compositor through shared GPU textures
- [Browser sources keep a profile](/quest/m0/browser-profile.md) - one persistent profile, so logins survive restarts, and a reload action
- [Browser source popups](/quest/m0/browser-popups.md) - dropdowns and other in-page popups draw and take input
- [Web studio](/quest/m0/web.md) - the full studio in wasm on WebGPU and WebCodecs, served from this repository
- [Virtual camera](/quest/m0/vcam/README.md) - a canvas's program exposed as a system camera on Linux, Windows, and macOS
- [Signing identities are available](/quest/m0/signing.md) - the Apple and Windows code-signing credentials packaging and the macOS camera need
- [Packaging](/quest/m0/packaging.md) - signed installers for macOS and Windows and a Flatpak for Linux
- [One audio capture feeding the bus and an encoder is pinned](/quest/m0/pin-audio-capture.md) - moq-audio's capture split, for audio source tracks
- [moq-mux's presentation timeline is pinned](/quest/m0/pin-presentation-timeline.md) - shared playout timing, for broadcast sync
- [moq-audio's mix router is pinned](/quest/m0/pin-audio-mix-router.md) - several mixes and mix-minus, for audio mixes
- [The persistable portal restore token is pinned](/quest/m0/pin-portal-token.md) - moq-video's restore token, for Wayland sources
- [Platform AAC encoders are pinned](/quest/m0/pin-aac-encode.md) - moq-audio's AAC encoders, for destinations
- [moq-audio's filter chain is pinned](/quest/m0/pin-audio-filters.md) - the chain audio filters configure
- [moq-video's browser backends are pinned](/quest/m0/pin-browser-video.md) - the web studio's video slice and browser composition
- [moq-audio's browser backends are pinned](/quest/m0/pin-browser-audio.md) - the web studio's audio slice
- [moq-video's program readback is pinned](/quest/m0/pin-readback.md) - the readback the Linux virtual camera uses

## Related

- [OBS native codecs](https://github.com/moq-dev/moq/blob/main/quest/m2/obs-moq-video/README.md) - the OBS plugin stays the path for creators who stay on OBS
- [Linux capture](https://github.com/moq-dev/moq/blob/main/quest/m2/capture-linux.md) - Wayland window, system audio, and app capture the studio's sources inherit
- [Windows capture](https://github.com/moq-dev/moq/blob/main/quest/m2/capture-windows.md) - app capture and system audio the studio's sources inherit
