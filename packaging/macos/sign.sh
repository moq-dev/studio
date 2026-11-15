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

# Nested code is signed before the bundle that holds it, innermost first, each
# with its own entitlements; the bundle's signature then seals theirs.
codesign "${flags[@]}" --entitlements "$packaging/entitlements.plist" "$app"
codesign --verify --strict --deep --verbose=2 "$app"
