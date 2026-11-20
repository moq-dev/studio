#!/usr/bin/env bash
# Sign an app bundle bundle.sh laid out, with the hardened runtime and the
# entitlements beside this script, as the identity given: a Developer ID
# Application certificate's SHA-1 for a release, or `-` to sign ad hoc, which
# runs the same runtime and entitlements without the release's secrets.
# Optional: the keychain that holds the identity.
set -euo pipefail

app=$1
identity=$2
keychain=${3:-}
packaging=$(cd "$(dirname "$0")" && pwd)

flags=(--force --options runtime --sign "$identity")
if [[ $identity == - ]]; then
    flags+=(--timestamp=none)
else
    # Notarization requires a secure timestamp.
    flags+=(--timestamp)
fi
if [[ -n $keychain ]]; then
    flags+=(--keychain "$keychain")
fi

# The entitlements each helper app's process needs, by its name: none but
# those that compile code at run time: the GPU and renderer helpers, and the
# plain one too, since CEF 156 starts pages' renderers and utility processes
# from it, and under the hardened runtime V8 cannot reserve its code range
# without `allow-jit`.
entitlements() {
    case $1 in
        *" Helper.app" | *" Helper (Renderer).app" | *" Helper (GPU).app") echo "$packaging/jit.entitlements.plist" ;;
        *.app) ;;
        *)
            echo "no entitlements known for $1" >&2
            return 1
            ;;
    esac
}

# Nested code is signed before the bundle that holds it, innermost first, each
# with its own entitlements; the bundle's signature then seals theirs. A
# framework's libraries go before the framework.
shopt -s nullglob
for framework in "$app"/Contents/Frameworks/*.framework; do
    for library in "$framework"/Libraries/*.dylib; do
        codesign "${flags[@]}" "$library"
    done
    codesign "${flags[@]}" "$framework"
done
for helper in "$app"/Contents/Frameworks/*.app; do
    plist=$(entitlements "$helper")
    codesign "${flags[@]}" ${plist:+--entitlements "$plist"} "$helper"
done
codesign "${flags[@]}" --entitlements "$packaging/entitlements.plist" "$app"
codesign --verify --strict --deep --verbose=2 "$app"
