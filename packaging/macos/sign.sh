#!/usr/bin/env bash
# Sign an app bundle bundle.sh laid out, with the hardened runtime and the
# entitlements beside this script, as the identity given: a Developer ID
# Application certificate's SHA-1 for a release, or `-` to sign ad hoc, which
# runs the same runtime and entitlements without the release's secrets.
# Optional: the keychain that holds the identity.
# A release bundle holds its Developer ID provisioning profile as
# Contents/embedded.provisionprofile, which grants the app the restricted
# entitlements installing the camera extension takes. An ad hoc signature
# cannot carry them, since macOS kills an app claiming them without a profile,
# so an ad hoc bundle runs but cannot install the extension.
set -euo pipefail
shopt -s nullglob

app=$1
identity=$2
keychain=${3:-}
packaging=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

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

fail() {
    echo "$1" >&2
    exit 1
}

# The app's entitlements: entitlements.plist, and for a release those its
# profile grants to install the camera extension, checked against the bundle.
app_entitlements=$packaging/entitlements.plist
if [[ $identity != - ]]; then
    profile=$app/Contents/embedded.provisionprofile
    [[ -f $profile ]] || fail "a release bundle needs its provisioning profile at $profile"
    security cms -D -i "$profile" >"$work/profile.plist"
    profile() {
        /usr/libexec/PlistBuddy -c "Print :$1" "$work/profile.plist"
    }
    team=$(profile TeamIdentifier:0)
    application=$(profile Entitlements:com.apple.application-identifier)
    [[ $application == "$team.live.kixel.studio" ]] ||
        fail "the profile is for $application, not $team.live.kixel.studio"
    [[ $(profile Entitlements:com.apple.developer.system-extension.install) == true ]] ||
        fail "the profile does not grant installing system extensions"
    # The extension's Mach service, and the app group that serves it, are
    # named for the team that signs it.
    extensions=("$app"/Contents/Library/SystemExtensions/*.systemextension)
    ((${#extensions[@]} == 1)) || fail "the bundle holds no single system extension"
    service=$(/usr/libexec/PlistBuddy -c "Print :CMIOExtension:CMIOExtensionMachServiceName" \
        "${extensions[0]}/Contents/Info.plist")
    [[ $service == "$team".* ]] || fail "the camera extension's service $service is not team $team's"

    app_entitlements=$work/entitlements.plist
    cp "$packaging/entitlements.plist" "$app_entitlements"
    /usr/libexec/PlistBuddy \
        -c "Add :com.apple.application-identifier string $application" \
        -c "Add :com.apple.developer.team-identifier string $team" \
        -c "Add :com.apple.developer.system-extension.install bool true" \
        "$app_entitlements"
fi

# The entitlements each nested bundle's process needs, by its name: for the
# helper apps, none but those that compile code at run time: the GPU and
# renderer helpers, and the plain one too, since CEF 156 starts pages'
# renderers and utility processes from it, and under the hardened runtime V8
# cannot reserve its code range without `allow-jit`. The camera extension's
# sandbox and app group.
entitlements() {
    case $1 in
        *" Helper.app" | *" Helper (Renderer).app" | *" Helper (GPU).app") echo "$packaging/jit.entitlements.plist" ;;
        *.app) ;;
        *.systemextension) echo "$packaging/camera.entitlements.plist" ;;
        *)
            echo "no entitlements known for $1" >&2
            return 1
            ;;
    esac
}

# Nested code is signed before the bundle that holds it, innermost first, each
# with its own entitlements; the bundle's signature then seals theirs. A
# framework's libraries go before the framework.
for framework in "$app"/Contents/Frameworks/*.framework; do
    for library in "$framework"/Libraries/*.dylib; do
        codesign "${flags[@]}" "$library"
    done
    codesign "${flags[@]}" "$framework"
done
for nested in "$app"/Contents/Frameworks/*.app "$app"/Contents/Library/SystemExtensions/*.systemextension; do
    plist=$(entitlements "$nested")
    codesign "${flags[@]}" ${plist:+--entitlements "$plist"} "$nested"
done
codesign "${flags[@]}" --entitlements "$app_entitlements" "$app"
codesign --verify --strict --deep --verbose=2 "$app"
