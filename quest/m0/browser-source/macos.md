# [M] Browser sources on macOS

## Goal

Browser sources run on macOS as the
[epic](/quest/m0/browser-source/README.md) describes, each page's shared
texture imported into wgpu as an IOSurface.

## Plan

On top of what Linux landed:

- The helper is CEF's browser process, so it needs an app bundle with CEF's
  helper apps (`cef-rs`'s `bundle-cef-app`) and its own NSApplication. They
  go into the bundle [the macOS app](/quest/m0/packaging/macos.md) owns, and
  through its signing, rather than a second bundle.
- An IOSurface crosses processes only as a Mach port
  (`IOSurfaceCreateMachPort`), so the channel needs a Mach port rendezvous.
- Exiting with the studio stays as on Linux: the helper kills the process
  group it leads.

## Required

- [macOS app](/quest/m0/packaging/macos.md) - the app bundle CEF's helper apps join
