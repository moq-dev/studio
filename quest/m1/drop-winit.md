# [M] Drops on Wayland, and links dragged from a browser

## Goal

A file dragged onto the picture on a Wayland desktop lands where it is let go,
as it does on Windows, macOS, and X11, and a web address dragged from a
browser's address bar or a link becomes a web page layer the same way. The
studio's own per-platform pointer lookup during a drag
(`rs/moq-studio/src/pointer.rs`) is gone.

## Plan

winit 0.30, which eframe 0.36 and 0.37 use, reports only file paths: no drops
at all on Wayland, no position on any platform (so the studio asks X11,
Win32, and AppKit for the pointer each frame of a drag), and no text or URL
drags, which X11 refuses as an "unexpected protocol" and the others never
register for. winit's next release replaces that with a drag API
(`WindowEvent::DragEntered`, `DragMoved`, `DragDropped`, `DragLeft`, and
`ActiveEventLoop::fetch_data_transfer`) that carries the position and any
data type, on every backend including Wayland.

- Once eframe runs on it, take drops from those events: the position from
  `DragMoved`, files and `text/uri-list` or `text/x-moz-url` web addresses
  from the data transfer, and delete `pointer.rs`.
- Recommended, not decided (see Waiting): no per-platform hook for web
  addresses meanwhile. Each platform would need its own drag target
  registered beside winit's (an `IDropTarget` taking `CF_UNICODETEXT`, an
  `NSView` drag type for URLs, and an Xdnd type list winit owns), which the
  new API makes redundant, while Ctrl+V already turns a copied address into a
  web page layer at the pointer.

## Waiting

- rust-windowing/winit: a release of the drag API (`0.31`, now
  `0.31.0-beta.4`) that eframe adopts; check `cargo search eframe` and that
  release's `egui-winit` dependency on `winit`. A 0.30 backport of Wayland
  drops ([rust-windowing/winit#4729](https://github.com/rust-windowing/winit/pull/4729))
  would bring Wayland drops alone, landing at the scene's center, sooner.
- Maintainer: whether web addresses dragged from a browser wait for that
  release (recommended: paste covers them, and a hook per platform is three
  drag targets beside winit's to delete later) or get a hook per platform
  now.

## Related

- [Adding a source by picking a picture](/quest/m0/ux/add.md) - its cards land where they are dropped, inside the window
