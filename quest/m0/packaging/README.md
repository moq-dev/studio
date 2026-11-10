# Studio packaging

## Goal

Every release ships a signed and notarized macOS app, an unsigned Windows
installer, and a Linux Flatpak of `moq-studio`, all built in CI. Each artifact
is verified to launch and list devices on its platform, and lands on its own.

## Plan

Decided 2026-10-26 (maintainer): as for the `moq` binary in
[Ship capture and playback](https://github.com/moq-dev/moq/blob/main/quest/m3/cli-packaging.md), moq-studio's defaults
skip any feature that is expensive to build or relies on a system library,
and a `release` feature turns on every backend its platform can load without
one (`capture`, `openh264`, `vaapi`, `nvidia`, and the platform frameworks).
Every artifact builds with `release`, and the first one to land adds the
feature. `pipewire` stays out of `release` because it links libpipewire at
load time.

Decided 2026-11-09 (maintainer, /quest-audit): ✅ one quest per platform
(rejected: Related links between the quests that shape the macOS bundle;
leaving one packaging quest). One quest held all three artifacts behind the
Apple identity and the Windows host, though the Flatpak needs neither, and
three quests shaped the macOS app bundle with no link between them. The
[macOS app](/quest/m0/packaging/macos.md) owns that bundle.

## Related

- [Ship capture and playback](https://github.com/moq-dev/moq/blob/main/quest/m3/cli-packaging.md) - the same distribution checks for the `moq` binary
