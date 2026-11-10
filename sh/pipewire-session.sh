#!/usr/bin/env bash
# Run a command inside a private PipeWire session: a PipeWire daemon and the
# WirePlumber session manager that links streams, on a runtime directory of
# their own, so a test reaches neither the desktop's session nor needs one.
set -euo pipefail

# The dev shell carries PipeWire for linking; the session manager comes from the
# same nixpkgs.
wireplumber=$(nix build --inputs-from . --no-link --print-out-paths 'nixpkgs#wireplumber^out')

# Only the packages' own configuration, never the host's, which can name
# components these builds do not ship.
pipewire_share=$(dirname "$(command -v pipewire)")/../share
export PIPEWIRE_CONFIG_DIR=$pipewire_share/pipewire
export WIREPLUMBER_CONFIG_DIR=$wireplumber/share/wireplumber
export WIREPLUMBER_DATA_DIR=$wireplumber/share/wireplumber

runtime=$(mktemp -d)
export XDG_RUNTIME_DIR=$runtime
unset PIPEWIRE_REMOTE DBUS_SESSION_BUS_ADDRESS
pids=()
cleanup() {
    if ((${#pids[@]})); then
        kill "${pids[@]}" 2>/dev/null || true
        wait "${pids[@]}" 2>/dev/null || true
    fi
    rm -rf "$runtime"
}
trap cleanup EXIT

pipewire &
pids+=("$!")
# The socket appears once the daemon listens; a client started before it fails.
until [[ -S $runtime/pipewire-0 ]]; do
    kill -0 "${pids[0]}"
    sleep 0.1
done
"$wireplumber/bin/wireplumber" &
pids+=("$!")
# WirePlumber connects as the session manager once it has loaded its policy.
until pw-cli ls Client 2>/dev/null | grep -q 'application.name = "WirePlumber"'; do
    kill -0 "${pids[1]}"
    sleep 0.1
done

"$@"
