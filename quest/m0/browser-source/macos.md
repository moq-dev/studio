# [M] Browser sources on macOS

## Goal

Browser sources run on macOS as the
[epic](/quest/m0/browser-source/README.md) describes, each page's shared
texture imported into wgpu as an IOSurface.

## Plan

On top of what Linux landed:

- The helper is CEF's browser process, so it needs an app bundle with CEF's
  helper apps (`cef-rs`'s `bundle-cef-app`) and its own NSApplication. They
  go into the studio's app bundle rather than a second one:
  `packaging/macos/bundle.sh` lays them out under `Contents/Frameworks`, and
  `packaging/macos/sign.sh` signs each, with the entitlements it needs, before
  the app that holds them.
- An IOSurface crosses processes only as a Mach port
  (`IOSurfaceCreateMachPort`), so the channel needs a Mach port rendezvous.
- Exiting with the studio stays as on Linux: the helper kills the process
  group it leads.
