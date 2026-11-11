# [M] App icon

## Goal

`moq-studio` has an app icon, drawn once and shown wherever the system
shows the app: the Flatpak's menu entry and window, and the macOS bundle's
Dock and Finder. Today it has none, so each shows a generic placeholder.

## Plan

Decided 2026-11-11 (maintainer, /quest-complete): ✅ one icon shared by the
Flatpak and the macOS bundle, planned as its own quest (rejected: leaving
each artifact without one).

One SVG source in `packaging/` is the only drawn copy; every other format
is generated from it in the build, never checked in.

- **Flatpak:** installed as
  `share/icons/hicolor/scalable/apps/live.kixel.studio.svg`, named by the
  desktop entry's `Icon=live.kixel.studio`. Add AppStream metainfo in the
  same change only if the software centers need it to show the icon.
- **macOS:** an `.icns` built from the SVG and named by the bundle's
  `Info.plist`, which the [macOS app](/quest/m0/packaging/macos.md) owns;
  whichever of the two lands second wires the icon into the bundle.
- **Window:** egui's viewport icon, for X11 and the platforms that read it
  from the window rather than the desktop entry.

Recommendation: draw from the [studio's look](/quest/m0/ux/kit.md)'s
palette, its program red on near-black, without waiting for that quest.
The [Windows installer](/quest/m0/packaging/windows.md) reuses the same
source as an `.ico`.

Human check: taste, at the sizes a menu, a Dock, and a title bar draw it.

## Related

- [macOS app](/quest/m0/packaging/macos.md) - the bundle that names the icon
- [The studio's look](/quest/m0/ux/kit.md) - the palette the icon draws from
