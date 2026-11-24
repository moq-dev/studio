kixel.live Studio: native GPU streaming studio compositing scenes (`rs/moq-scene`, UI-free wgpu compositor) and egui app (`rs/moq-studio`) over MoQ (`moq/` submodule).

# Required

- Measure performance by frame time, GPU copies, and encoder load. Measure UI frame time under a real compositor (`WLR_BACKENDS=headless sway`), never Xvfb (which caps present at 12-25 fps on NVIDIA hosts).
- Keep `moq-scene` UI-free (no egui types) so headless compositors and wasm can reuse it.
- Keep the window simple: any PR adding visible UI explains why most streamers need it; niche features live behind `Settings > Mods`, off by default and hidden everywhere until enabled.
- `just shots` renders named window views (`rs/moq-studio/src/shots.rs`); update/add views for UI changes and attach WebP comparisons (`cwebp -q 80`) in the PR body.

# Upstream (`moq/`)

- Media, capture, codecs, audio, catalog, and transport work lands in `moq-dev/moq` first (with a moq-side test), then bumps the `moq` gitlink via `just upgrade-moq`. Never edit `moq/` here or point the submodule off moq's `main` (`sh/check-moq-main.sh`).
- Every moq crate is a path dep into `moq/` in root `Cargo.toml` (`sh/check-moq-pin.sh`). Follow `moq/rs/AGENTS.md` (especially `kio::clock`, banned direct clock reads in `clippy.toml`, and never branching on negotiated protocol version).

# Development

PRs target `main`. Run `just ci` before pushing (outside the dev shell, `nix develop --command just ...`). Update `README.md` in the same PR when changing user-facing flags, panels, scene files, or outputs.

Quests: before any work, run `quest context` and follow it, unless this session already holds its rules.
The `quest` binary comes from the kixel-ai/quest flake input; change skills upstream and bump the input.
