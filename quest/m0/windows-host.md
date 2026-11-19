# [S] The windows PC hosts Windows agents

## Goal

An agent running natively on the maintainer's Windows PC (Tailscale name
`windows`) builds `moq-studio --features capture` there and lists a camera
and a microphone to capture, so Windows quests run on real hardware.

## Plan

Decided 2026-11-10 (maintainer): ✅ the `windows` PC is the Windows host
(rejected: rebooting nixos2 into its Windows partition, the 2026-11-06 plan).
Agents (Claude Code, Codex) already run on it natively through T3 Code, so no
ssh hop is needed, and nixos2 stays a Linux host.

The host, as found on 2026-11-10:

- Hostname `WINDOWS`, Tailscale `windows` (100.73.17.32), Windows 11 Home
  10.0.26200, i7-8700, 16 GB RAM, NVIDIA RTX 2070 (driver 32.0.15.9186), one
  2560x1440 display.
- Toolchain: rustup with `stable-x86_64-pc-windows-msvc` as default, plus
  1.98.1, which rustup installed from `rust-toolchain.toml` on first use;
  Visual Studio Build Tools 2022 17.14 with the VC x86/x64 tools; CMake 4.3;
  Git 2.43; just 1.52; `quest` 0.2.0 built from source (no Nix). No nasm or
  clang, which nothing has yet shown the build needs.
- Capture devices: no camera. Audio inputs are virtual only: Steam Streaming
  Microphone and VB-Audio Virtual Cable (`CABLE Output` records what is
  played to `CABLE Input`).
- SSH (2026-11-12): an OpenSSH server runs (ED25519 host key) but rejects the
  Linux host's key (publickey denied), so other hosts cannot reach it by ssh;
  agents run on it directly instead.

No real microphone is needed: VB-Audio's `CABLE Output` is a WASAPI capture
endpoint like a USB microphone's, so playing a tone into `CABLE Input` drives
the same capture path [Studio on Windows](/quest/m0/app-windows.md) runs, with
a known signal to check. A camera has no such stand-in.

There is no Nix dev shell, so agents here run cargo and `just` natively;
`just check` steps that call Nix tools cannot run.

The remaining agent step: build with
`quest gate cargo build -p moq-studio --features capture`, then list devices
by starting the studio, whose setup and Devices panel enumerate cameras,
displays, windows, and audio inputs (`rs/moq-studio/src/devices.rs`).
Whatever fails to build on this host is this quest's to fix.

Decided 2026-11-12 (maintainer, /quest-review checkpoint): ✅ this quest owns
every build error on the host (rejected: handing errors CI also shows to the
CI build quest, which would make that quest this one's prerequisite; merging
that split as is).

Found 2026-11-15 (maintainer, /quest-complete): the `windows` PC is dead (it
kept crash looping), so the host above no longer exists and the two earlier
Waiting items (free 4 GiB for the gate; plug in a USB camera) no longer apply.
The quest needs a new Windows host; the maintainer has not picked one. The
Goal and host notes above describe the dead PC and stay until `/quest-plan`
replans this quest around the new host.

## Hosts

- windows - the work is setting up this host

## Waiting

- Maintainer: pick a new Windows host and replan this quest with
  `/quest-plan`. Options: a cloud or GitHub-hosted Windows machine, a Windows
  VM on another host, or new hardware. Not picked here: the maintainer
  chooses in `/quest-plan`. Check: an agent on the new host builds
  `moq-studio --features capture` and the studio's Devices panel lists a
  camera and a microphone.
