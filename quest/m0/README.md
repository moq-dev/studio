# m0: the studio

## Goal

A native, cross-platform streaming studio (`rs/moq-studio`) for creators and
teams who rely on OBS today. It composites
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
  transforms, transitions, and canvases on wgpu. The program audio bus is
  moq-audio's `bus` module. Both grow slice by slice with the
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
  Changed 2026-11-09 (maintainer, /quest-audit): ✅ the
  [web studio](/quest/m2/web.md) and its two pins moved to m2 (rejected:
  keeping them in m0), so m0 ships the native studio alone. moq removed all
  WASM support on 2026-10-31, its browser-backend quests are deferred with
  no other consumer, and nothing plans a browser WebTransport session for
  moq-net.
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
  [viewer-side composition](/quest/m2/compose/README.md) epic (now m2) moved whole:
  the server composite is a headless binary here rather
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
[viewer-side composition](/quest/m2/compose/README.md) epic left m0 (now m2,
replanned 2026-11-08 around independently published sources layered over the
program). Overlays and animation render into the program here; that epic's
layout track carries only the layers of independently published sources.

Dropped from the first scoping round: relay fan-out ("encode once, the relay
restreams") and its upload savings. Direct push with shared encoders matches
OBS multistream, and the hosted version is product work outside this
repository.

## Required

- [nixos2 boots Windows for agents](/quest/m0/windows-host.md) - a Windows machine agents reach over ssh, with a camera and microphone
- [The studio builds for Windows in CI](/quest/m0/windows-build.md) - a nightly `windows-latest` build, which needs no Windows host
- [Studio on Windows](/quest/m0/app-windows.md) - the studio runs on Windows, not only Linux
- [Wayland display and window sources](/quest/m0/wayland-sources.md) - portal-picked sources that reopen without the picker
- [Destinations](/quest/m0/destinations.md) - RTMP, RTMPS, and WHIP push with per-destination codecs and shared encoders, on a UI-free egress library a hosted service can reuse
- [AAC for destinations](/quest/m0/destinations-aac.md) - AAC for the ingests that require it, from the platform encoders
- [Clap-to-measure sync delay](/quest/m0/sync-clap.md) - a clap sets a microphone's delay against its camera
- [Audio filters](/quest/m0/audio-filters.md) - noise suppression, gate, compressor, limiter, and plugins on each microphone and system audio source, configured from the studio
- [Browser sources on macOS and Windows](/quest/m0/browser-source/README.md) - the out-of-process CEF pages Linux has, with shared GPU textures on the other desktops
- [Virtual camera](/quest/m0/vcam/README.md) - the program or a published scene exposed as a system camera on Linux, Windows, and macOS
- [The Apple signing identity is available](/quest/m0/signing.md) - the Apple credentials the macOS app and its camera need; Windows ships unsigned
- [Packaging](/quest/m0/packaging/README.md) - a signed macOS app, a Windows installer, and a Flatpak for Linux, each its own quest
- [The persistable portal restore token is pinned](/quest/m0/pin-portal-token.md) - moq-video's restore token, for Wayland sources
- [Platform AAC encoders are pinned](/quest/m0/pin-aac-encode.md) - moq-audio's AAC encoders, for AAC destinations
- [moq-audio's filter chain is pinned](/quest/m0/pin-audio-filters.md) - the chain audio filters configure
- [moq's egress dial changes are pinned](/quest/m0/pin-egress-dial.md) - RTMPS that keeps `tcUrl` and a caller-supplied dial policy, for destinations
- [moq-audio's source tap is pinned](/quest/m0/pin-audio-tap.md) - the samples clap-to-measure sync times a clap from
- [Playout writes land on positioned bus channels](/quest/m0/playout-positioned.md) - broadcast, media, and replay sound at the bus position its pictures are timed for
- [moq-audio's positioned bus channel is pinned](/quest/m0/pin-bus-positioned.md) - `Bus::positioned`, which playout writes through
- [moq-mux's decoupled playout clock is pinned](/quest/m0/pin-sync.md) - `moq_mux::sync` in place of `moq_mux::timeline`, which every later pin crosses
- [The studio is caught up to moq's main](/quest/m0/pin-main.md) - moq's breaking changes past #8272 in one PR, so each later pin stays small

## Related

- [OBS native codecs](https://github.com/moq-dev/moq/blob/main/quest/m3/obs-moq-video/README.md) - the OBS plugin stays the path for creators who stay on OBS
- [Linux capture](https://github.com/moq-dev/moq/blob/main/quest/m3/capture-linux.md) - Wayland window, system audio, and app capture the studio's sources inherit
- [Windows capture](https://github.com/moq-dev/moq/blob/main/quest/m3/capture-windows.md) - app capture and system audio the studio's sources inherit
