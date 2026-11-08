# [S] nixos2 boots Windows for agents

## Goal

nixos2 boots Windows by default, and an agent on the maintainer's Mac reaches
it with `ssh nixos2`, builds `moq-studio --features capture` there, and finds
a camera and a microphone to capture, so Windows quests run on real hardware.

## Plan

Decided 2026-11-06 (maintainer, /quest-unblock): nixos2 becomes the Windows
host. Decided 2026-11-07 (maintainer, /quest-unblock): it no longer waits for
a Wayland trial on its Intel GPU, which was skipped.

nixos2 already dual-boots: Windows sits on its own partition (`nvme0n1p3`,
600 GB, last booted 2026-09-14) with firmware entry `Boot0003` (Windows Boot
Manager); NixOS boots through systemd-boot on `nvme0n1p4`. The switch:

1. From NixOS, an agent moves `Boot0003` to the front of the firmware boot
   order (`efibootmgr -o`, with passwordless sudo) and reboots. NixOS stays
   installed; boot it once with `bcdedit /set {fwbootmgr} bootsequence
   {<entry>}` from Windows when a Linux run needs it.
2. The maintainer turns on OpenSSH Server (see Waiting).
3. Over ssh, an agent installs the toolchain with `winget`: rustup with the
   MSVC target, Visual Studio Build Tools with the C++ workload, Git, and
   `just`. It records the host key change for `nixos2` and what it installed
   in this PR.

## Waiting

- Maintainer: at nixos2's console once it boots Windows, turn on OpenSSH
  Server (Settings > System > Optional features), add the Mac's public key to
  `C:\ProgramData\ssh\administrators_authorized_keys`, and plug in a USB
  camera and a microphone, which nixos2 lacks. An agent cannot reach Windows
  before sshd runs. Check: `ssh nixos2 ver` prints a Windows version.
