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

Root cause, found 2026-11-15:
- #163 moved the `moq` pin past
  [moq-dev/moq#8695](https://github.com/moq-dev/moq/pull/8695), a dependabot
  bump that put moq-video's portals on `ashpd` 0.14 and `zbus` 6.
- zbus 6 writes the whole D-Bus handshake at once. xdg-dbus-proxy before 0.1.6
  (Ubuntu 24.04 LTS ships 0.1.5, and Flatpak runs the host's proxy) stops
  parsing the bus's answers once it forwards `BEGIN`, so it drops the
  connection.
- This reproduces against a locally built 0.1.5 proxy, and 0.1.9 accepts it.
- The wgpu panic is not a second bug: the smoke script tears down sway the
  moment the camera listing fails, while the studio is still opening its
  window.

The fix is [moq-dev/moq#8861](https://github.com/moq-dev/moq/pull/8861). It
holds `ashpd` at 0.13 (zbus 5, which stages its handshake under
`FLATPAK_ID`), with a test that plays the old proxy.

Once it merges:
1. Run `just upgrade-moq` to a moq main that includes it.
2. Merge the API catch-up staged on the `validate/flatpak-cameras` branch,
   commits `ba2638e` and `b891e41`. It covers moq-dev/moq#8638's groups
   addressed by their start and the fallible `Timestamp`, plus #8742's
   `DisplaySelection`.
3. Check that the Flatpak `Bundle` jobs pass.

The studio's own `ashpd` is already 0.13, so the tree keeps one `zbus`.

## Hosts

- linux - the Flatpak and its sandbox

## Waiting

- moq-dev/moq: [moq-dev/moq#8861](https://github.com/moq-dev/moq/pull/8861) merges on moq's `main`. Check that the PR is merged, then advance the pin.

## Related

- [Studio packaging](/quest/m0/packaging/README.md) - the release workflow that ships the Flatpak
