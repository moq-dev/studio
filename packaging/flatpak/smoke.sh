#!/usr/bin/env bash
# Launch the installed Flatpak as its menu entry does, on a headless sway with
# private D-Bus and PipeWire sessions, and check that it lists cameras through
# the camera portal and audio through PipeWire, then quits cleanly when its
# window closes. Needs flatpak, sway, pipewire, wireplumber, and
# xdg-desktop-portal on the host.
set -euo pipefail

# The portal starts on the session bus when the studio first calls it.
if [[ -z ${STUDIO_SMOKE:-} ]]; then
    exec env STUDIO_SMOKE=1 dbus-run-session -- "$0" "$@"
fi

runtime=$(mktemp -d)
export XDG_RUNTIME_DIR=$runtime
log=$runtime/studio.log
pids=()
cleanup() {
    if ((${#pids[@]})); then
        kill "${pids[@]}" 2>/dev/null || true
        wait "${pids[@]}" 2>/dev/null || true
    fi
    rm -rf "$runtime"
}
trap cleanup EXIT

# Wait for a socket, failing as soon as the daemon that should create it exits.
await() {
    until compgen -G "$2" >/dev/null; do
        kill -0 "$1"
        sleep 0.1
    done
}

pipewire &
pids+=("$!")
await "$!" "$runtime/pipewire-0"
wireplumber &
pids+=("$!")

WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 sway --config /dev/null &
pids+=("$!")
await "$!" "$runtime/sway-ipc.*.sock"
await "$!" "$runtime/wayland-*[0-9]"
SWAYSOCK=$(compgen -G "$runtime/sway-ipc.*.sock")
WAYLAND_DISPLAY=$(basename "$(compgen -G "$runtime/wayland-*[0-9]")")
export SWAYSOCK WAYLAND_DISPLAY

flatpak run --env=NO_COLOR=1 --command=moq-studio-desktop live.kixel.studio 2>"$log" &
studio=$!

# Guided setup lists the devices on the first launch. The log ends when the
# studio exits, and the timeout ends a launch that hangs.
status=0
timeout 300 grep -m1 -q 'listed devices' <(tail --pid="$studio" -n +1 -F "$log" 2>/dev/null) || status=$?
if ((status == 0)); then
    swaymsg '[app_id="moq-studio"] kill' >/dev/null
    wait "$studio" || status=$?
else
    kill "$studio" 2>/dev/null || true
fi
cat "$log"
((status == 0)) || {
    echo "the studio did not list devices and quit cleanly (status $status)" >&2
    exit 1
}

listing=$(grep 'listed devices' "$log")
for kind in cameras audio; do
    [[ $listing == *"$kind=Ok("* ]] || {
        echo "the studio could not list $kind" >&2
        exit 1
    }
done
