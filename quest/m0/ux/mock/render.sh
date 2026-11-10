#!/usr/bin/env bash
# Render frames of a mock-up page to <page>-<frame>.webp beside it, at the 1440x900 they are drawn for:
#   ./render.sh shell live build portrait
set -euo pipefail
cd "$(dirname "$0")"
page=$1
shift
for frame in "$@"; do
	"${CHROME:-google-chrome}" --headless=new --disable-gpu --hide-scrollbars --window-size=1440,900 \
		--virtual-time-budget=4000 --screenshot="$PWD/$page-$frame.webp" "file://$PWD/$page.html#shot:$frame" >/dev/null 2>&1
done
