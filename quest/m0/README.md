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
- **UI:** egui, without egui_dock. It has same-device zero-copy wgpu
  textures, multiple native windows, and a proven wasm build; gpui has no wgpu
  on macOS or Windows, and Tauri is a webview. The app owns the wgpu device.
  Decided again 2026-11-11 by the
  [spike](https://github.com/moq-dev/studio/pull/126) of the LIVE page:
  ✅ egui (rejected: Slint, defer). egui's UI CPU was about 0.5 ms a frame,
  against Slint's 12 to 16 ms as released and 3 to 5 ms with femtovg's cache
  patched; patched Slint also quadrupled the program render, and Slint's
  licenses demand GPLv3 or attribution with an embedded ban.
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

Changed 2026-11-09 with the maintainer (/quest-plan): the studio gets one
designed layout, a live picture with pages around it, in place of sixteen
dockable panels; see [Broadcaster UX](/quest/m0/ux/README.md). Docking no
longer argues for a toolkit, so that epic's spike decided it again between
egui and Slint, the two that draw the compositor's textures with no copy on
the studio's own device: egui stays (above).

Dropped from the first scoping round: relay fan-out ("encode once, the relay
restreams") and its upload savings. Direct push with shared encoders matches
OBS multistream, and the hosted version is product work outside this
repository.

## Related

- [OBS native codecs](https://github.com/moq-dev/moq/blob/main/quest/m3/obs-moq-video/README.md) - the OBS plugin stays the path for creators who stay on OBS
- [Linux capture](https://github.com/moq-dev/moq/blob/main/quest/m3/capture-linux.md) - Wayland window, system audio, and app capture the studio's sources inherit
- [Windows capture](https://github.com/moq-dev/moq/blob/main/quest/m3/capture-windows.md) - app capture and system audio the studio's sources inherit
