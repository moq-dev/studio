# [XS] moq-audio's positioned bus channel is pinned

## Goal

The `moq` submodule includes moq-audio's `Bus::positioned`, whose
`bus::Positioned::write(position, data)` plays a write at the bus position it
names, from [moq-dev/moq#8273](https://github.com/moq-dev/moq/pull/8273). That
PR also renamed `Bus::channel`/`bus::Channel` to `Bus::live`/`bus::Live`.

## Plan

Planned 2026-11-08 (maintainer, moq's /quest-complete), a follow-up of
moq-dev/moq#8273, which has merged on moq's `main`.

Decided 2026-11-09 (maintainer, /quest-audit): ✅ pin exactly #8273, at
`79960a9dd` with `just upgrade-moq 79960a9dd` (rejected: bump to moq `main`
here, which crosses the playout clock's replacement below and resizes this
quest).

Land it in its own PR, fixing whatever the new API breaks here: at least the
`Bus::live` rename at every `bus.channel(..)` call.
[moq-dev/moq#8272](https://github.com/moq-dev/moq/pull/8272) merged after
#8273, as `644d48996`. It replaces `moq_mux::timeline` with `moq_mux::sync`
and is not crossed here; [pin-sync](/quest/m0/pin-sync.md) crosses it. Move no
audio onto positioned writes here; that is
[Playout writes land on positioned bus channels](/quest/m0/playout-positioned.md).
