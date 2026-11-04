# [L] Browser sources on macOS and Windows

## Goal

Browser sources run on macOS and Windows as they do on Linux: a URL (an
overlay, alert box, or chat widget) renders into the compositor at the canvas
frame rate, the creator can interact with it, its audio joins the bus, and a
crash drops browser sources, never the stream: a page's renderer only its own
source, the shared browser process every browser source.

## Plan

Decided 2026-10-22: use CEF through `cef-rs`, desktop only.
- **Rejected engines:** Servo's site compatibility is not yet enough for
  common overlay sites. Blitz runs no JavaScript.
- **wasm build:** omits browser sources until a web API can draw HTML into a
  canvas.
- **Rendering:** CEF offscreen rendering with a shared texture
  (`OnAcceleratedPaint`), imported into wgpu as a D3D11 shared handle on
  Windows, an IOSurface on macOS, and a dmabuf on Linux. The CPU `OnPaint`
  path is the fallback and shows in stats.
- **Process:** CEF runs out of process.

Linux landed first (`moq_scene::browser`): one helper process every page
shares, on one persistent profile, which is the studio's executable started
again, talking over a Unix socket on its stdin that carries the DMA-BUF
descriptors, and `cef-rs`'s `osr_texture_import` copies each texture on the
studio's device. The profile lives in the XDG data directory
(`engine::browser_profile`); each platform keeps it in its own per-user data
directory instead. What the other platforms need on top:
- **Windows:** a channel without Unix sockets, such as the helper's stdio
  pipes. The studio duplicates CEF's NT handle out of the helper with
  `DuplicateHandle` before `cef-rs` imports it.
- **macOS:** the helper is CEF's browser process, so it needs an app bundle
  with CEF's helper apps (`cef-rs`'s `bundle-cef-app`) and its own
  NSApplication. An IOSurface crosses processes only as a Mach port
  (`IOSurfaceCreateMachPort`), so the channel needs a Mach port rendezvous.

## Required

- [Studio on Windows](/quest/m0/app-windows.md) - the studio runs there at all; it already runs on macOS

## Related

- [Studio on macOS with capture granted](/quest/m0/app-macos.md) - the rest of the macOS app, which pages do not need
