# [XS] moq's egress dial changes are pinned

## Goal

The `moq` submodule includes the two moq changes destinations dial through:
moq-rtmp's `rtmps://` dial keeps the URL-derived `tcUrl`, from
[RTMPS keeps `tcUrl`](https://github.com/moq-dev/moq/blob/main/quest/m3/rtmp-rtmps-tcurl.md),
and a dial takes a policy its caller supplies, so a host can refuse private
and loopback addresses, from
[a caller-supplied dial policy](https://github.com/moq-dev/moq/blob/main/quest/m3/webrtc-dial-policy.md).

## Plan

Decided 2026-11-09 (maintainer, /quest-audit): ✅ both changes are planned in
moq's tree with this pin here (rejected: planning them inline in
[Destinations](/quest/m0/destinations.md), with no moq quest or pin).

Once the wait clears, advance it with `just upgrade-moq` in its own PR,
fixing whatever the new API breaks here.

## Waiting

- moq-dev/moq: both upstream quests above land. They were being planned in
  moq's tree on 2026-11-09, so each link resolves once its plan merges.
  Check: both quests' PRs have merged on moq's `main`, deleting their files,
  moq-rtmp's TLS dial sends the URL-derived `tcUrl`, and a dial takes the
  caller's policy. A quest file that moved, or went away without its work
  landing, does not count.

## Required

- [The studio is caught up to moq's main](/quest/m0/pin-main.md) - moq's breaking changes since moq-dev/moq#8272, so this pin takes only its own
