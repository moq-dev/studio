set shell := ["bash", "-euo", "pipefail", "-c"]

# The device, codec, and browser features off by default, which every check
# covers too.
features := "moq-scene/capture,moq-studio/capture,moq-studio/openh264,moq-studio/pipewire,moq-studio/cef"

# List the available development commands.
default:
    @just --list

# Lint the repository tooling, the quest tree, the join page, and the workspace.
check:
    sh/check-moq-pin.sh
    quest check
    actionlint
    shellcheck .claude/hooks/*.sh .codex/hooks/*.sh sh/*.sh packaging/flatpak/*.sh
    shfmt -d .claude/hooks/*.sh .codex/hooks/*.sh sh/*.sh packaging/flatpak/*.sh
    taplo format --check
    nixfmt --check flake.nix
    bun install --frozen-lockfile
    bun biome check .
    bun run check
    bun run --cwd js/join build
    cargo fmt --all --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo clippy --locked --workspace --all-targets --features {{ features }} -- -D warnings
    # The studio as `cargo run -p moq-studio` builds it: alone, so the features
    # only its tests' dependencies turn on cannot hide one it needs.
    cargo check --locked -p moq-studio --features {{ features }}

# Run the tests.
[positional-arguments]
test *args:
    bash .claude/hooks/direnv.test.sh
    bun run test
    cargo nextest run --locked --workspace --features {{ features }} "$@"

# Run what CI's Check runs: the lints, then the tests.
ci:
    just check
    just test

# Apply formatters without changing program behavior.
fix:
    shfmt -w .claude/hooks/*.sh .codex/hooks/*.sh sh/*.sh packaging/flatpak/*.sh
    taplo format
    nixfmt flake.nix
    bun biome check --write .
    cargo fmt --all

# Composite scenes, render browser sources, publish a program, and show it as a
# PipeWire camera through wgpu on Mesa's software Vulkan driver, so no GPU is
# needed.
render-sw:
    #!/usr/bin/env bash
    set -euo pipefail
    mesa=$(nix build --inputs-from . --no-link --print-out-paths nixpkgs#mesa)
    icd=("$mesa"/share/vulkan/icd.d/lvp_icd.*.json)
    export VK_DRIVER_FILES="${icd[0]}"
    cargo nextest run --locked -p moq-scene --lib --run-ignored only -E 'test(/^compositor::tests::/) | test(/^browser::layers::tests::/)'
    cargo nextest run --locked -p moq-studio --features openh264 --run-ignored only -E '(test(/^engine::tests::/) - test(=engine::tests::the_virtual_camera_reaches_a_pipewire_app)) | test(=setup::tests::times_every_canvas) | test(=watch::tests::plays_a_guest_where_the_studio_did)'
    cargo nextest run --locked -p moq-studio --features cef --run-ignored only -E 'binary(browser)'
    sh/pipewire-session.sh cargo nextest run --locked -p moq-studio --features pipewire --run-ignored only -E 'test(=engine::tests::the_virtual_camera_reaches_a_pipewire_app)'

# Show the program through a v4l2loopback device, which needs the module loaded
# (`sudo modprobe v4l2loopback exclusive_caps=1`) and the device writable.
v4l2loopback:
    cargo nextest run --locked -p moq-studio --run-ignored only -E 'test(=vcam::v4l2::tests::a_v4l2_app_reads_the_program)'

# Move the moq submodule to moq's latest main (or a given commit) and refresh Cargo.lock and bun.lock.
upgrade-moq rev="origin/main":
    sh/upgrade-moq.sh {{ rev }}

# Serve the join page locally while working on it.
join:
    bun install --frozen-lockfile
    bun run --cwd js/join dev
