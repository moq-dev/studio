# [S] The studio builds for Windows in CI

## Goal

`moq-studio` builds for `x86_64-pc-windows-msvc` with `--features capture` on
a `windows-latest` job in the nightly workflow
(`.github/workflows/nightly.yml`), and whatever fails to compile there is
fixed.

## Plan

Decided 2026-11-09 (maintainer, /quest-audit): ✅ its own quest (rejected:
folding it into [Studio on Windows](/quest/m0/app-windows.md)). A hosted
runner compiles it, so it needs no Windows host and is not blocked behind
one.

Nothing has compiled the whole crate for Windows.
[moq-dev/studio#84](https://github.com/moq-dev/studio/pull/84) reported that
`cargo check` for the target fails off a Windows host, in C build scripts
(`tract-linalg`) that need an MSVC toolchain, so it checked only
`hotkeys/native.rs` and `hotkeys/layout.rs`, in a scratch crate.

The job is nightly, beside the macOS one, not on the merge gate. Windows has
no Nix dev shell, so the job installs the toolchain `rust-toolchain.toml`
names itself.
