# [M] The new Windows PC hosts Windows agents

## Goal

A new Windows 11 Home desktop, reachable over Tailscale as `windows`, runs
agents natively through T3 Code. On it an agent builds
`moq-studio --features capture`, runs the studio, lists a real camera and a
microphone, and publishes through both the Intel (QSV) and NVIDIA (NVENC)
Media Foundation encoders, so Windows quests run on real hardware.

## Plan

Decided 2026-11-25 (maintainer, `/quest-plan`):

- Host: ✅ a new desktop, bought 2026-11-25 and due 2026-12-01: an ABS
  Flux II Aqua prebuilt with an Intel Core Ultra 7 260K (Arc iGPU, whose
  only output on the MSI PRO B860M board is HDMI), an RTX 5060 Ti 16 GB,
  32 GB, and Windows 11 Home (rejected: an Azure
  GPU VM, with no real camera or display and Windows Server unless licensed
  for Windows 11; reinstalling the old i7-8700/RTX 2070 PC, 2017 hardware
  that may crash again; Windows on nixos as its OS or as a VFIO guest, which
  puts CI at risk and covers NVIDIA only).
- Windows edition: ✅ Home (rejected: the $99 Pro upgrade now). Agents reach
  the host over ssh and T3 Code, which Home supports; Pro adds an RDP host
  and Windows Update deferral policy. Home upgrades to Pro later from
  Settings > System > Activation for the same price if either is needed.
- Why two vendors: on Windows `moq-video` encodes through each vendor's
  Media Foundation encoder, which differ in the rate control they accept
  (moq's [Capped VBR on MediaFoundation](https://github.com/moq-dev/moq/blob/main/quest/m1/video-rate/windows.md)).
  This box covers Intel's and NVIDIA's, capture on one adapter with encode on
  the other, and AV1 NVENC, which no other host has. AMD's Windows encoder
  stays untested: ephemeral Azure VMs for it were dropped.
- Camera: ✅ a spare UVC webcam the maintainer owns. Microphone: a USB one if
  at hand, otherwise VB-Audio Virtual Cable, whose `CABLE Output` is a WASAPI
  capture endpoint like a USB microphone's, so playing a tone into
  `CABLE Input` drives the same capture path with a known signal.
- Display: the old PC's monitor, or an HDMI dummy plug if headless, so
  Windows keeps a display to capture.
- Earlier, kept: ✅ this quest owns every build error on the host (rejected:
  handing errors CI also shows to a CI build quest).

Guidance, from the old host (i7-8700, 16 GB, Windows 11 Home, dead since
2026-11-15): rustup with `stable-x86_64-pc-windows-msvc` and the pinned
toolchain from `rust-toolchain.toml`, Visual Studio Build Tools with the VC
x86/x64 tools, CMake, Git, `just`, and `quest` built from source. There is no
Nix dev shell, so `just check` steps that call Nix tools cannot run. The old
host's OpenSSH server rejected the Linux hosts' keys; this one accepts them,
so other hosts reach it by ssh. Rename the dead `windows` node in Tailscale
and give the new one that name, so existing references keep working. Add the
host to dotfiles' `HOSTS.md` in a dotfiles PR.

The agent steps once the maintainer has it on Tailscale with T3 Code: install
the toolchain, build with `quest gate cargo build -p moq-studio --features
capture`, start the studio and check that Add (`rs/moq-studio/src/devices.rs`
enumerates its sources) lists the camera, displays, windows, and the
microphone, then publish once per vendor. Media Foundation uses the first
hardware encoder that activates, so each run leaves only one vendor's
encoder available (for example by disabling the other adapter in Device
Manager) and records which GPU encoded the stream. The PR records the host
facts above, refreshed.

## Hosts

- windows - the work is setting up this host

## Waiting

- Maintainer (PC): once the PC arrives (due 2026-12-01), finish Windows
  11 Home's first sign-in, sign in to Tailscale as `windows` and to T3 Code,
  enable OpenSSH with the Linux hosts' keys, and plug in the webcam and a
  display (HDMI on the motherboard for the iGPU). An agent cannot handle
  hardware or the first Windows sign-in. Check: `ssh windows ver` answers
  from another host.
