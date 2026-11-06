# [S] One Vulkan loader for the studio and CEF

## Goal

On NixOS, `cargo run`, `cargo nextest` and an installed studio find the GPU
through the system Vulkan loader, with no `VK_DRIVER_FILES` or
`LD_LIBRARY_PATH` set, because the build no longer ships CEF's bundled
`libvulkan.so.1` beside the binaries.

## Plan

From [moq-dev/studio#59](https://github.com/moq-dev/studio/pull/59), picked
2026-11-05 (maintainer, /quest-plan), m0 by the rule that fixes join m0.

- ✅ **Stop copying CEF's `libvulkan.so.1`** into the target directory, so
  the studio and CEF's GPU process both load the system loader. Rejected:
  exporting `VK_DRIVER_FILES` from the dev shell, which fixes development but
  not a packaged build.
- Today the copy in `target/debug` loads first, finds no NixOS drivers, and
  wgpu gets no adapter. The `render-sw` recipe in the `justfile` works around it
  by exporting `LD_LIBRARY_PATH` and `VK_DRIVER_FILES`. Drop the workaround if it
  is no longer needed for the software driver.
- Verify on NixOS (`ssh nixos`) that CEF's GPU path (Wayland, shared
  textures) and its SwiftShader CPU path still start, that WebGL still draws,
  and that the GPU tests find an adapter under plain `cargo nextest`.
