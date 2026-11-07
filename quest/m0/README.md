# m0: the studio

## Goal

A native, cross-platform streaming studio (`rs/moq-studio`) for creators and
teams who rely on OBS today, plus the same app built to wasm. It composites
cameras, screens, media, browser sources, and remote MoQ guests into scenes
on the GPU with moq-video and moq-audio, publishing several scenes at once
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
  run a server. Destinations that take the same picture scene and audio scene with
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
  [viewer-side composition](/quest/m1/compose/README.md) epic (now m1) moved whole:
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

Changed 2026-11-04 with the maintainer: the program is how a studio
broadcast reaches viewers, not the fallback. Sources are not published to the
relay by default (decided 2026-11-03 on
[moq-dev/studio#21](https://github.com/moq-dev/studio/pull/21)), because
each encoding costs an encoder session and consumer GPUs have few, so the
[viewer-side composition](/quest/m1/compose/README.md) epic moved to m1.
Overlays and animation render into the program here, and travel to viewers
in the layout track only once that epic lands.

Dropped from the first scoping round: relay fan-out ("encode once, the relay
restreams") and its upload savings. Direct push with shared encoders matches
OBS multistream, and the hosted version is product work outside this
repository.

## Required

- [Studio on macOS with capture granted](/quest/m0/app-macos.md) - camera, screen, and audio capture run on a Mac, not only Linux
- [nixos2 boots Windows for agents](/quest/m0/windows-host.md) - a Windows machine agents reach over ssh, with a camera and microphone
- [Studio on Windows](/quest/m0/app-windows.md) - the studio runs on Windows, not only Linux
- [Wayland display and window sources](/quest/m0/wayland-sources.md) - portal-picked sources that reopen without the picker
- [Destinations](/quest/m0/destinations.md) - RTMP, RTMPS, and WHIP push with per-destination codecs and shared encoders, on a UI-free egress library a hosted service can reuse
- [Pan each sound](/quest/m0/audio-pan.md) - the Audio panel and `[audio.<source>]` pan each sound wherever it plays
- [Clap-to-measure sync delay](/quest/m0/sync-clap.md) - a clap sets a microphone's delay against its camera
- [Audio filters](/quest/m0/audio-filters.md) - per-source noise suppression, gate, compressor, limiter, and plugins, configured from the studio
- [Browser sources on macOS and Windows](/quest/m0/browser-source.md) - the out-of-process CEF pages Linux has, with shared GPU textures on the other desktops
- [Web studio](/quest/m0/web.md) - the full studio in wasm on WebGPU and WebCodecs, served from this repository
- [Virtual camera](/quest/m0/vcam/README.md) - the program or a published scene exposed as a system camera on Linux, Windows, and macOS
- [Browser sources on the GPU under Mesa](/quest/m0/cef-headless.md) - shared-texture pages paint on Intel and AMD under Wayland, and a page that cannot paint fails loudly
- [The program reaches a relay on every runner](/quest/m0/relay-test-flake.md) - the relay engine test stops failing the nightly on GitHub's runner
- [Browser sources make no outside requests](/quest/m0/browser-offline.md) - no component updates, spellcheck downloads, or network time from the helper
- [The Apple signing identity is available](/quest/m0/signing.md) - the Apple credentials packaging and the macOS camera need; Windows ships unsigned
- [Packaging](/quest/m0/packaging.md) - a signed macOS app, a Windows installer, and a Flatpak for Linux
- [The persistable portal restore token is pinned](/quest/m0/pin-portal-token.md) - moq-video's restore token, for Wayland sources
- [Platform AAC encoders are pinned](/quest/m0/pin-aac-encode.md) - moq-audio's AAC encoders, for destinations
- [moq-audio's filter chain is pinned](/quest/m0/pin-audio-filters.md) - the chain audio filters configure
- [moq-video's browser backends are pinned](/quest/m0/pin-browser-video.md) - the web studio's video slice and browser composition
- [moq-audio's browser backends are pinned](/quest/m0/pin-browser-audio.md) - the web studio's audio slice
- [moq-audio's source tap is pinned](/quest/m0/pin-audio-tap.md) - the samples clap-to-measure sync times a clap from
- [moq-audio's strip pan is pinned](/quest/m0/pin-audio-pan.md) - the bus strip pan the studio's pan sets

## Related

- [OBS native codecs](https://github.com/moq-dev/moq/blob/main/quest/m3/obs-moq-video/README.md) - the OBS plugin stays the path for creators who stay on OBS
- [Linux capture](https://github.com/moq-dev/moq/blob/main/quest/m3/capture-linux.md) - Wayland window, system audio, and app capture the studio's sources inherit
- [Windows capture](https://github.com/moq-dev/moq/blob/main/quest/m3/capture-windows.md) - app capture and system audio the studio's sources inherit
