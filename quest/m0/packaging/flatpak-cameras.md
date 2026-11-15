# [M] The Flatpak lists cameras again

## Goal

The Flatpak's launch check (`packaging/flatpak/smoke.sh`) passes again on
main: inside the sandbox the studio lists cameras through the camera portal
instead of failing its D-Bus handshake.

## Plan

Found 2026-11-15 on moq-dev/studio#170. The Flatpak's `Bundle` jobs fail on
main itself ([run](https://github.com/moq-dev/studio/actions/runs/43708081575),
dispatched on `354ee7d`), with `listed devices cameras=Err("camera portal:
ZBus Error: D-Bus handshake failed: Unexpected EOF during handshake")`, then a
wgpu surface panic. The last green Nightly ran on `a73fb69`.

The likely cause is #163's catch-up to moq's main: moq-video's camera portal
moved to `ashpd` 0.14 on `zbus` 6.3, while the studio's own portals
(`global_shortcuts`, `settings`) stay on `ashpd` 0.13 and `zbus` 5. Reproduce
the handshake with `zbus` 6 in a Flatpak sandbox against the session bus
proxy, then fix it where it lies. A `zbus` or moq-video fix lands in
moq-dev/moq first, with a moq-side test, and a `moq` bump here; aligning the
studio's own `ashpd` with moq's then keeps one `zbus`.

Whether the wgpu panic ("The surface isn't supported by this adapter") is a
second failure or follows the first is part of the investigation; the smoke
test stops at the camera listing before it matters.

## Hosts

- linux - the Flatpak and its sandbox

## Related

- [Studio packaging](/quest/m0/packaging/README.md) - the release workflow that ships the Flatpak
