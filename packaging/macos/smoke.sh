#!/usr/bin/env bash
# Launch the app bundle given as Finder does, through Launch Services with no
# arguments, and check that it lists cameras and audio, opens its window, and
# quits when asked without crashing. A fresh home directory makes it open a
# new scene collection, so guided setup lists the devices.
set -euo pipefail

app=$(cd "$1" && pwd)
executable=$app/Contents/MacOS/moq-studio
home=$(mktemp -d)
log=$home/studio.log
touch "$log"
pid=

cleanup() {
    if [[ -n $pid ]]; then
        kill "$pid" 2>/dev/null || true
    fi
    echo "--- the studio's log"
    cat "$log"
    rm -rf "$home"
}
trap cleanup EXIT

fail() {
    echo "$1" >&2
    exit 1
}

# Whether the studio has a window on screen. Listing windows by owner needs
# no permission, unlike reading their titles.
window() {
    osascript -l JavaScript - "$pid" <<'JS'
ObjC.import("CoreGraphics");
function run(argv) {
	const windows = ObjC.deepUnwrap(ObjC.castRefToObject(
		$.CGWindowListCopyWindowInfo($.kCGWindowListOptionOnScreenOnly, $.kCGNullWindowID)));
	return windows.some((w) => w.kCGWindowOwnerPID === Number(argv[0]) && w.kCGWindowLayer === 0);
}
JS
}

# A crash leaves a report here, not in the log.
reports=~/Library/Logs/DiagnosticReports
marker=$home/started
touch "$marker"

open -n "$app" --stdout "$log" --stderr "$log" --env HOME="$home" --env NO_COLOR=1
deadline=$((SECONDS + 30))
until pid=$(pgrep -nfx "$executable"); do
    ((SECONDS < deadline)) || fail "the studio never started"
    sleep 0.1
done

# Guided setup lists the devices before its window opens.
deadline=$((SECONDS + 300))
until grep -q 'listed devices' "$log"; do
    kill -0 "$pid" 2>/dev/null || fail "the studio exited before listing its devices"
    ((SECONDS < deadline)) || fail "the studio never listed its devices"
    sleep 0.1
done
listing=$(grep 'listed devices' "$log")
for kind in cameras audio; do
    [[ $listing == *"$kind=Ok("* ]] || fail "the studio could not list $kind"
done

deadline=$((SECONDS + 120))
until [[ $(window) == true ]]; do
    kill -0 "$pid" 2>/dev/null || fail "the studio exited before its window opened"
    ((SECONDS < deadline)) || fail "the studio's window never opened"
    sleep 0.5
done

# Quit as the menu, the Dock, and logging out do.
osascript -e 'tell application id "live.kixel.studio" to quit'
deadline=$((SECONDS + 60))
while kill -0 "$pid" 2>/dev/null; do
    ((SECONDS < deadline)) || fail "the studio did not quit when asked"
    sleep 0.1
done
pid=
crashed=$(find "$reports" -name 'moq-studio*' -newer "$marker" 2>/dev/null)
[[ -z $crashed ]] || fail "the studio crashed: $crashed"
! grep -q 'panicked' "$log" || fail "the studio panicked"
