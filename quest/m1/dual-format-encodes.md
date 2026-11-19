# [S] Time two program encodes for real

## Goal

Setup and the go-live sheet say whether this machine keeps up with a show
going out in landscape and portrait at once from two encodes timed together,
not from one encode's time doubled. The numbers come from a real GPU encoder
with its session cap, such as NVENC on `nixos`'s RTX 3070 Ti, beside the
software encoders.

## Plan

Decided 2026-11-17 (maintainer, on
[moq-dev/studio#200](https://github.com/moq-dev/studio/pull/200)): ✅
estimate two encodes as twice one until this lands (rejected: timing two
encodes in that PR).

- `setup::Encoding::keeps_up_all` and `total_of` double one encode's frame
  time and bitrate. Time the landscape and the turned canvas encoding at once
  instead, as `setup::time` times one, and keep the doubled estimate only
  where two sessions cannot open.
- A check that opens two hardware sessions holds both while it runs, so it
  must finish before going live opens the program's, as the one-encode check
  already does.
- Measure on `nixos` (NVENC, session cap 8, with CARLA and a transcode job
  holding some) and on a software encoder: frame time and encoder load for
  one encode and for two, per AGENTS.md. Record whether doubling over- or
  under-estimates, and by how much.

## Hosts

- linux - the RTX 3070 Ti on `nixos`
