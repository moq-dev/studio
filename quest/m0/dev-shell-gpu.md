# [S] The dev shell runs the studio on a non-NixOS GPU

## Goal

On a non-NixOS Linux host with an AMD or Intel GPU, `nix develop --command
cargo run -p moq-studio --features capture,openh264,pipewire,vaapi` opens the
window on the GPU and encodes through VA-API, with no environment set by hand.
An NVIDIA host keeps its own driver, as today.

## Plan

Found 2026-11-25 on the maintainer's Fedora desktop (Radeon, GNOME Wayland):

- No GPU at all: the Nix-built Vulkan loader finds the host's ICD files, but
  Fedora's `libvulkan_radeon.so` cannot load its own libraries (`libdrm`,
  `libLLVM`) from a Nix binary, so the studio failed with "no GPU adapter".
  Pointing `VK_DRIVER_FILES` at Nix Mesa's `radeon_icd.x86_64.json` fixed it.
- No VA-API: `runtimeLibs` in `flake.nix` lacks `libva`, so `moq-vaapi`'s
  dlopen fails and every encoder falls back to openh264. Adding Nix's `libva`
  to `LD_LIBRARY_PATH`, with `LIBVA_DRIVERS_PATH` at Nix Mesa's `lib/dri`,
  opened H.264 on `renderD128`.
- Fedora's own Mesa leaves out the H.264 and H.265 encoders in both VA-API
  and Vulkan Video, so Nix Mesa's drivers are the ones that encode here.

Recommended: add `libva` to `runtimeLibs`, and in the shell hook, when the
host has no `/run/opengl-driver` (not NixOS) and no NVIDIA driver, point
`VK_DRIVER_FILES`, `LIBVA_DRIVERS_PATH`, and `GBM_BACKENDS_PATH` at Nix
Mesa's, the Mesa from the flake's nixpkgs. Decide this from the host at shell
setup, not by trying at runtime. Validate on this host: the log shows
`opened VA-API H264 encoder` and no `wgpu_hal` loader errors.
