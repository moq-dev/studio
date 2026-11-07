#!/usr/bin/env bash
# Check out Rune at the rev the workspace pins, with `rune.patch` applied, where
# the root Cargo.toml's [patch] points (quest/m1/rune-spike.md). The patch adds
# `rune::adopt_gpu`, so Rune draws on the studio's device, and lets rune-taffy
# build when moq-scene's blitz enables taffy's `flexbox_balance`.
#
# To measure as the spike did, on a Linux host: copy this directory's scripts
# into `.tmp/`, put the libxkbcommon and wayland lib dirs in `.tmp/libs.sh` as
# `RUNE_LIBS`, copy `show.toml` and its media into `.tmp/bench/`, run
# `instrument.py` and `instrument2.py` (temporary egui timing), `build.sh`,
# then `runs.sh` inside `nix shell nixpkgs#sway nixpkgs#grim`.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
rev=b99c7b04e75c0b7587fe8d09ba6a679e11ede9c9
dir="$root/.tmp/rune"
if [ ! -d "$dir/.git" ]; then
	git clone --quiet --filter=blob:none https://github.com/anomalyco/rune "$dir"
fi
git -C "$dir" checkout --quiet --force "$rev"
git -C "$dir" apply "$root/sh/rune-spike/rune.patch"
