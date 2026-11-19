#!/usr/bin/env bash
# Build moq-studio with the `release` feature for this Mac's architecture and
# lay it out as an app bundle at the path given, unsigned; sign.sh signs it.
# Needs Xcode's command line tools, cargo, and jq.
set -euo pipefail

app=$1
packaging=$(cd "$(dirname "$0")" && pwd)
cd "$packaging/../.."

# Info.plist's LSMinimumSystemVersion, so the linker and OpenH264's C build
# target the oldest macOS the bundle claims.
export MACOSX_DEPLOYMENT_TARGET=13.0
json=$(cargo build --locked --release -p moq-studio --features release --message-format=json-render-diagnostics)
# The binary, and the build script's output, which holds the icon it drew.
executable=$(jq -r 'select(.reason == "compiler-artifact" and .target.name == "moq-studio" and .executable != null) | .executable' <<<"$json")
out=$(jq -r 'select(.reason == "build-script-executed" and (.package_id | test("/rs/moq-studio#"))) | .out_dir' <<<"$json")
version=$(cargo metadata --locked --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "moq-studio") | .version')
[[ -x $executable && -f $out/live.kixel.studio.icns && -n $version ]]

# Nested code, such as helpers, frameworks, and system extensions, goes under
# Contents in the directory macOS expects it in, and sign.sh signs it first.
rm -rf "$app"
# Readable by every user, whatever this checkout's own modes.
umask 022
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$packaging/Info.plist" "$app/Contents/Info.plist"
chmod 644 "$app/Contents/Info.plist"
plutil -replace CFBundleShortVersionString -string "$version" "$app/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$version" "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
cp "$executable" "$app/Contents/MacOS/moq-studio"
cp "$out/live.kixel.studio.icns" "$app/Contents/Resources/live.kixel.studio.icns"
