#!/usr/bin/env bash
# Build moq-studio with the `release` and `cef` features for this Mac's
# architecture and lay it out as an app bundle at the path given, unsigned;
# sign.sh signs it. Needs Xcode's command line tools, cargo, CMake, Ninja,
# and jq.
set -euo pipefail

app=$1
packaging=$(cd "$(dirname "$0")" && pwd)
cd "$packaging/../.."

# Info.plist's LSMinimumSystemVersion, so the linker and OpenH264's C build
# target the oldest macOS the bundle claims.
export MACOSX_DEPLOYMENT_TARGET=13.0
# The studio, the binary CEF's subprocesses run from (browser sources), and
# the virtual camera's extension.
json=$(cargo build --locked --release -p moq-studio -p moq-scene -p moq-studio-camera \
    --bin moq-studio --bin moq-browser-helper --bin moq-studio-camera \
    --features moq-studio/release,moq-studio/cef,moq-scene/cef --message-format=json-render-diagnostics)
# The binaries, the build script's output, which holds the icon it drew, and
# cef-dll-sys's, where it downloads CEF unless CEF_PATH names a copy.
artifact() {
    jq -r --arg name "$1" 'select(.reason == "compiler-artifact" and .target.name == $name and .executable != null) | .executable' <<<"$json"
}
executable=$(artifact moq-studio)
helper=$(artifact moq-browser-helper)
camera=$(artifact moq-studio-camera)
out=$(jq -r 'select(.reason == "build-script-executed" and (.package_id | test("/rs/moq-studio#"))) | .out_dir' <<<"$json")
cef=$(jq -r 'select(.reason == "build-script-executed" and (.package_id | test("cef-dll-sys"))) | .out_dir' <<<"$json")
version=$(cargo metadata --locked --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "moq-studio") | .version')
[[ -x $executable && -x $helper && -x $camera && -f $out/live.kixel.studio.icns && -n $version ]]
frameworks=()
while IFS= read -r framework; do
    frameworks+=("$framework")
done < <(find "${CEF_PATH:-$cef}" -maxdepth 3 -type d -name "Chromium Embedded Framework.framework")
((${#frameworks[@]} == 1))

# Nested code, such as helpers, frameworks, and system extensions, goes under
# Contents in the directory macOS expects it in, and sign.sh signs it first.
rm -rf "$app"
# Readable by every user, whatever this checkout's own modes.
umask 022
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks"
cp "$packaging/Info.plist" "$app/Contents/Info.plist"
chmod 644 "$app/Contents/Info.plist"
plutil -replace CFBundleShortVersionString -string "$version" "$app/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$version" "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
cp "$executable" "$app/Contents/MacOS/moq-studio"
cp "$out/live.kixel.studio.icns" "$app/Contents/Resources/live.kixel.studio.icns"

# CEF, which the studio's executable loads when it starts again as the
# browser-source helper, and the helper apps CEF starts its subprocesses
# from: `<executable> Helper.app` and its variants, each named and identified
# as CEF looks for them.
ditto "${frameworks[0]}" "$app/Contents/Frameworks/Chromium Embedded Framework.framework"
for variant in "" " (GPU)" " (Renderer)" " (Plugin)" " (Alerts)"; do
    name="moq-studio Helper$variant"
    contents="$app/Contents/Frameworks/$name.app/Contents"
    mkdir -p "$contents/MacOS"
    cp "$packaging/Helper.plist" "$contents/Info.plist"
    chmod 644 "$contents/Info.plist"
    # `(GPU)` names `live.kixel.studio.helper.gpu`.
    suffix=$(tr -d ' ()' <<<"$variant" | tr '[:upper:]' '[:lower:]')
    plutil -replace CFBundleExecutable -string "$name" "$contents/Info.plist"
    plutil -replace CFBundleIdentifier -string "live.kixel.studio.helper${suffix:+.$suffix}" "$contents/Info.plist"
    plutil -replace CFBundleName -string "$name" "$contents/Info.plist"
    plutil -replace CFBundleShortVersionString -string "$version" "$contents/Info.plist"
    plutil -replace CFBundleVersion -string "$version" "$contents/Info.plist"
    plutil -lint "$contents/Info.plist"
    cp "$helper" "$contents/MacOS/$name"
done

# The virtual camera's Camera Extension, which the studio asks the system to
# activate from here. Named for its identifier, as the system requires.
contents="$app/Contents/Library/SystemExtensions/live.kixel.studio.camera.systemextension/Contents"
mkdir -p "$contents/MacOS"
cp "$packaging/Camera.plist" "$contents/Info.plist"
chmod 644 "$contents/Info.plist"
plutil -replace CFBundleShortVersionString -string "$version" "$contents/Info.plist"
plutil -replace CFBundleVersion -string "$version" "$contents/Info.plist"
plutil -lint "$contents/Info.plist"
cp "$camera" "$contents/MacOS/moq-studio-camera"
