kixel.live Studio is a native, cross-platform streaming studio that composites scenes on the GPU and goes live over MoQ (Media over QUIC).
`rs/moq-scene` is the UI-free scene graph and wgpu compositor; `rs/moq-studio` is the egui app. Both build on the moq crates in the `moq` submodule.

`moq/` is moq-dev/moq at a pinned commit. Its own `AGENTS.md` files apply to the moq crates you read there, but never edit them here; see Upstream below.

# Required

- Measure performance by frame time, GPU copies, and encoder load, and the UI's frame time under a real compositor, such as headless sway (`WLR_BACKENDS=headless sway`), not Xvfb, whose present caps it at 12–25 fps on the NVIDIA test hosts.

# Upstream

Media, transport, and protocol work belongs in moq-dev/moq, not here: capture, codecs, rendering import and export (moq-video), audio capture, mixing, and filters (moq-audio), the catalog (hang), and the transport (moq-net, moq-tokio).
A studio feature that needs a moq change lands that change in moq first, with a moq-side test proving it, then bumps the `moq` gitlink here.
Never point the submodule at an unmerged moq branch on `main`.

# Rust

- Every moq crate is a path dependency into `moq/`, declared once in the root `Cargo.toml`. `sh/check-moq-pin.sh` fails `just check` if a moq git dependency appears or the checkout drifts from the gitlink.
- Read the clock through `kio::clock` (`clippy.toml` bans direct reads), so tests can pause it.
- `moq-scene` stays UI-free: no egui types, so a headless compositor and the wasm build can reuse it.
- Defaults skip anything needing a system library (`capture`, `pipewire`, `vaapi`, `openh264`); `just check` covers them explicitly.

# Development

PRs target `main`.
Run `just ci` before every push.

`README.md` is the user guide. A change to what a user does (flags, panels, the scene file, outputs) updates it in the same PR.

Quests: before any work, run `quest context` and follow it, unless this session already holds its rules.
The `quest` binary comes from the moq-dev/quest flake input, so outside the dev shell run it as `nix develop --command quest ...`. It serves the quest skills; change them upstream and bump the input.
