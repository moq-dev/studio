#!/usr/bin/env bash
# Launch the installed Flatpak as its menu entry does, on a headless sway with
# private D-Bus and PipeWire sessions, and check that it lists cameras through
# the camera portal and audio through PipeWire, then quits cleanly when its
# window closes. Needs flatpak, sway, pipewire, wireplumber, and
# xdg-desktop-portal on the host.
set -euo pipefail

# The portal starts on the session bus when the studio first calls it, and
# finds PipeWire in the runtime directory the bus was started with.
if [[ -z ${STUDIO_SMOKE:-} ]]; then
    XDG_RUNTIME_DIR=$(mktemp -d)
    export XDG_RUNTIME_DIR STUDIO_SMOKE=1
    trap 'rm -rf "$XDG_RUNTIME_DIR"' EXIT
    dbus-run-session -- "$0" "$@"
    exit
fi

runtime=$XDG_RUNTIME_DIR
log=$runtime/studio.log
pids=()
cleanup() {
    if ((${#pids[@]})); then
        kill "${pids[@]}" 2>/dev/null || true
        wait "${pids[@]}" 2>/dev/null || true
    fi
    if [[ -f $log ]]; then
        echo "--- the studio's log"
        cat "$log"
    fi
}
trap cleanup EXIT

fail() {
    echo "$1" >&2
    exit 1
}

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
pids+=("$studio")

# Guided setup lists the devices on the first launch. The log ends when the
# studio exits, and the timeout ends a launch that hangs.
timeout 300 grep -m1 -q 'listed devices' <(tail --pid="$studio" -n +1 -F "$log" 2>/dev/null) ||
    fail "the studio never listed its devices"
listing=$(grep 'listed devices' "$log")
for kind in cameras audio; do
    [[ $listing == *"$kind=Ok("* ]] || fail "the studio could not list $kind"
done

# Setup lists the devices before its window opens.
deadline=$((SECONDS + 120))
until swaymsg -t get_tree | grep -q '"app_id": "moq-studio"'; do
    kill -0 "$studio" || fail "the studio exited before its window opened"
    ((SECONDS < deadline)) || fail "the studio's window never opened"
    sleep 0.1
done
swaymsg '[app_id="moq-studio"] kill' >/dev/null
wait "$studio" || fail "the studio did not quit cleanly when its window closed"
