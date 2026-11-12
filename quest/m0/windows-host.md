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
a known signal to check. A camera has no such stand-in (see Waiting).

There is no Nix dev shell, so agents here run cargo and `just` natively;
`just check` steps that call Nix tools cannot run.

The remaining agent step: build with
`quest gate cargo build -p moq-studio --features capture`, then list devices
by starting the studio, whose setup and Devices panel enumerate cameras,
displays, windows, and audio inputs (`rs/moq-studio/src/devices.rs`).
[moq-dev/studio#129](https://github.com/moq-dev/studio/pull/129) adds a
nightly Windows job that compiles the studio in CI, and its run on
windows-latest built it. A build error that job also shows is fixed in CI's
quest or PR; one only this host shows is this quest's to fix.

## Hosts

- windows - the work is setting up this host

## Waiting

- Maintainer: leave 4 GiB of memory available on `windows` while an agent
  builds there. `quest gate` admits a build only once available memory
  reaches min(4 GiB, RAM/4), and with the desktop apps running (OneDrive's
  sync service alone held 3 GB) the PC sat at 2.6 to 3.1 GB available for two
  hours on 2026-11-10, so the build never started. An agent cannot close the
  maintainer's apps, and building outside the gate is against the repo's
  rules. Options: quit OneDrive, ChatGPT, and Discord during agent runs, or add
  RAM (recommended: add RAM, since 16 GB leaves a linker little room beside a
  desktop). Check: `(Get-Counter '\Memory\Available MBytes').CounterSamples`
  reads above 4096.
- Maintainer: plug a USB camera into `windows`. It has none, and Media
  Foundation's camera source, the path [Studio on Windows](/quest/m0/app-windows.md)
  runs, needs a device; no virtual camera ships with Windows. An agent could
  register a Media Foundation virtual camera (`MFCreateVirtualCamera`) only
  with a media source of its own, which is
  [the Windows virtual camera](/quest/m0/vcam/windows.md)'s deliverable and
  not built yet; a USB camera also covers the formats real hardware sends.
  Check: the studio's Devices panel lists a camera.
