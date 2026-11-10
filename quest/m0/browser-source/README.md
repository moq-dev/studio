# Browser sources on macOS and Windows

## Goal

Browser sources run on macOS and Windows as they do on Linux: a URL (an
overlay, alert box, or chat widget) renders into the compositor at the canvas
frame rate, the creator can interact with it, its audio joins the bus, and a
crash drops browser sources, never the stream: a page's renderer only its own
source, the shared browser process every browser source. Each platform lands
on its own.

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
directory instead.

The studio holds the channel open until the helper exits and stops it with
`Command::Stop`, so the helper reads the channel's end, from before CEF
starts, as the studio dying, and kills the process group it leads with CEF's
processes. Each platform's quest says what it needs on top.

Decided 2026-11-09 (maintainer, /quest-audit): ✅ one quest per platform
(rejected: Related links only; leaving one quest). One quest held macOS
behind the Windows host, and shaped the macOS app bundle with no link to the
[quest that owns it](/quest/m0/packaging/macos.md).
