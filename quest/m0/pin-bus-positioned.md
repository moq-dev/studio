# [XS] moq-audio's positioned bus channel is pinned

## Goal

The `moq` submodule includes moq-audio's `Bus::positioned`, whose
`bus::Positioned::write(position, data)` plays a write at the bus position it
names, from [moq-dev/moq#8273](https://github.com/moq-dev/moq/pull/8273). That
PR also renamed `Bus::channel`/`bus::Channel` to `Bus::live`/`bus::Live`.

## Plan

Planned 2026-11-08 (maintainer, moq's /quest-complete), a follow-up of
moq-dev/moq#8273, which has merged on moq's `main`.

Advance it with `just upgrade-moq` in its own PR, fixing whatever the new API
breaks here: at least the `Bus::live` rename at every `bus.channel(..)` call.
If [moq-dev/moq#8272](https://github.com/moq-dev/moq/pull/8272) has landed by
then, `moq_mux::timeline` is `moq_mux::sync`, and `playout.rs` follows that
PR's migration notes too. Move no audio onto positioned writes here; that is
[Playout writes land on positioned bus channels](/quest/m0/playout-positioned.md).
