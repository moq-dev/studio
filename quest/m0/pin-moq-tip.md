# [M] The studio catches up to moq's main

## Goal

The `moq` submodule points at moq's current `main`, past
[moq-dev/moq#8931](https://github.com/moq-dev/moq/pull/8931), with the
studio building on it: the health grade and failover keep working, through
public moq APIs rather than a workaround.

## Plan

Decided 2026-11-18 (maintainer, on
[moq-dev/studio#208](https://github.com/moq-dev/studio/pull/208)): ✅ land
#208's pin at `81e43217c` and catch up to the tip here (rejected: holding
#208 for the tip).

Once the wait clears, run `just upgrade-moq` in its own PR and absorb the
tip's breaks, as tried on #208:

- `encode::Producer` became `encode::Publisher`
  ([moq-dev/moq#8964](https://github.com/moq-dev/moq/pull/8964)).
- `Info::with_refresh` became a `Refresh`
  ([moq-dev/moq#9054](https://github.com/moq-dev/moq/pull/9054)), so
  `ladder::Joins` is rewritten on `Demand::poll_refresh`.
- Route `Preference` became `Priority`, `Key::to_public` became `public`, and
  `Demand::wanted` became a bool.
- `Stream::check_failover` (`rs/moq-studio/src/stream.rs`) stops inspecting
  the protocol version and its one-`--connect-version` rule goes: failover
  relies on moq-net to resume across relays. The rule's `--failover` doc
  (`rs/moq-studio/src/main.rs`) and its two README lines (under the flags and
  Destinations' **Failover relay**) go with it. The health grade (`health.rs`,
  `engine/air.rs`, `setup.rs`, `fx.rs`) moves to the republished
  `hang::health`.

Decided 2026-11-20 (maintainer, /quest-plan): ✅ the studio never inspects
the negotiated moq-net version; moq-net handles failover transparently, and if
a failover relay then restarts viewers rather than resuming them, moq-net's API
changes in a moq quest of its own (rejected: a public `lite::Version` instance
query; a session capability query; matching the version string).

## Waiting

- moq-dev/moq: `hang::health` is public again, as
  [Client health in use](https://github.com/moq-dev/moq/blob/main/quest/m3/stats/health-consumers.md)
  plans ([moq-dev/moq#8956](https://github.com/moq-dev/moq/pull/8956) made it
  private). Check: `rs/hang/src/lib.rs` on moq's `main` exports `health`.
