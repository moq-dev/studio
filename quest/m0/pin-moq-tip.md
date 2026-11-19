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
- `Stream::check_failover` (`rs/moq-studio/src/stream.rs`) moves to moq's
  public `lite::Version` instance query, and the health grade
  (`health.rs`, `engine/air.rs`, `setup.rs`, `fx.rs`) to the republished
  `hang::health`.

Matching the version string instead of the instance query is rejected: it
hard-codes `moq-lite-07-wip`.

## Waiting

- moq-dev/moq: `hang::health` is public again, as
  [Client health in use](https://github.com/moq-dev/moq/blob/main/quest/m3/stats/health-consumers.md)
  plans ([moq-dev/moq#8956](https://github.com/moq-dev/moq/pull/8956) made it
  private). Check: `rs/hang/src/lib.rs` on moq's `main` exports `health`.
- moq-dev/moq: a public way to ask whether a `lite::Version` carries
  instances ([moq-dev/moq#8931](https://github.com/moq-dev/moq/pull/8931) made
  `has_instance` crate-private). Its quest is pending planning in moq's m1, so
  it does not gate moq's 0.22.0 cut. Check: moq's `main` has a public query
  `Stream::check_failover` can call.
