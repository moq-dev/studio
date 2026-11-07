#!/usr/bin/env bash
# On nixos2, inside `nix shell nixpkgs#sway nixpkgs#grim`: run the studio for
# <seconds> as a client of a headless sway paced at 60 Hz, as #96 measured egui,
# then screenshot the output and print the bench lines.
# Usage: .tmp/bench.sh <seconds> <log> [env...]
set -u
root=$(cd "$(dirname "$0")/.." && pwd)
seconds=$1; log=$2; shift 2
. "$root/.tmp/libs.sh"
export XDG_RUNTIME_DIR=/run/user/$(id -u) XDG_DATA_HOME=$root/.tmp/xdg
export LD_LIBRARY_PATH=$RUNE_LIBS:${LD_LIBRARY_PATH:-}:/run/opengl-driver/lib
unset DISPLAY WAYLAND_DISPLAY
mkdir -p "$XDG_DATA_HOME"
printf 'output HEADLESS-1 mode 1600x1000@60Hz\n' > "$root/.tmp/sway.conf"
WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=gles2 WLR_RENDER_DRM_DEVICE=/dev/dri/renderD128 \
	sway --unsupported-gpu -d -c "$root/.tmp/sway.conf" > "$root/.tmp/$log.sway.log" 2>&1 &
sway=$!
sleep 3
display=$(grep -oE "WAYLAND_DISPLAY=wayland-[0-9]+|wayland display '[^']+'" "$root/.tmp/$log.sway.log" | head -1 | grep -oE 'wayland-[0-9]+')
cd "$root/.tmp/bench"
env "$@" WAYLAND_DISPLAY=$display RUST_LOG=info,wgpu=warn,naga=warn "$root/.tmp/target/release/moq-studio" show.toml > "$root/.tmp/$log.log" 2>&1 &
app=$!
sleep "$seconds"
WAYLAND_DISPLAY=$display grim "$root/.tmp/$log.png" 2>/dev/null
kill $app; wait $app
kill $sway; wait $sway 2>/dev/null
# The bench lines past the first ten seconds' warm-up.
sed 's/\x1b\[[0-9;]*m//g' "$root/.tmp/$log.log" | grep -E "bench|ERROR|panicked" | awk 'NR > 0' | cut -c28-400 > "$root/.tmp/$log.bench"
grep -c "" "$root/.tmp/$log.bench"
