# [M] The new Windows PC hosts Windows agents

## Goal

A new Windows 11 Pro desktop, reachable over Tailscale as `windows`, runs
agents natively through T3 Code. On it an agent builds
`moq-studio --features capture`, runs the studio, lists a real camera and a
microphone, and publishes through both the Intel (QSV) and NVIDIA (NVENC)
Media Foundation encoders, so Windows quests run on real hardware.

## Plan

Decided 2026-11-25 (maintainer, `/quest-plan`):

- Host: ✅ a new desktop with an Intel Core Ultra 5 245K-class CPU (Arc
  iGPU), an RTX 5060 Ti 16 GB, 32 GB, and Windows 11 Pro (rejected: an Azure
  GPU VM, with no real camera or display and Windows Server unless licensed
  for Windows 11; reinstalling the old i7-8700/RTX 2070 PC, 2017 hardware
  that may crash again; Windows on nixos as its OS or as a VFIO guest, which
  puts CI at risk and covers NVIDIA only). The maintainer buys it through
  [kixelated/corp's Windows PC quest](https://github.com/kixelated/corp/blob/main/quest/m0/windows-pc.md).
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
capture`, start the studio and check its setup and Devices panel
(`rs/moq-studio/src/devices.rs`) list the camera, displays, windows, and the
microphone, then publish once with each GPU's encoder. The PR records the host
facts above, refreshed.

## Hosts

- windows - the work is setting up this host

## Waiting

- Maintainer (PC): buy the PC
  ([corp quest](https://github.com/kixelated/corp/blob/main/quest/m0/windows-pc.md)),
  assemble it if it is a parts build, install Windows 11 Pro, sign in to
  Tailscale as `windows` and to T3 Code, enable OpenSSH with the Linux hosts'
  keys, and plug in the webcam and a display. An agent cannot handle
  hardware or the first Windows sign-in. Check: `ssh windows ver` answers
  from another host.
