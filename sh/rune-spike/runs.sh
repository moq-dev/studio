#!/usr/bin/env bash
# On nixos2, inside `nix develop` and `nix shell nixpkgs#sway nixpkgs#grim`:
# alternate egui (full dock), egui (monitors only) and the Rune spike, idle and
# recording, twice each.
set -u
cd "$(dirname "$0")"
for round in 1 2; do
	for load in idle rec; do
		extra=()
		[ "$load" = rec ] && extra=(BENCH_RECORD=1)
		bash bench.sh 45 "egui-full-$load-$round" "${extra[@]}"
		bash bench.sh 45 "egui-mon-$load-$round" BENCH_MONITORS=1 "${extra[@]}"
		bash bench.sh 45 "rune-$load-$round" STUDIO_UI=rune "${extra[@]}"
	done
done
echo done
