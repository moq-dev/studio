#!/usr/bin/env bash
# Pack an app bundle into a disk image at the path given, laid out for
# dragging to Applications. Optional: a Developer ID Application identity
# (its SHA-1) to sign the image with, and the keychain that holds it.
# Needs Python 3.10 or later.
set -euo pipefail

app=$1
dmg=$2
identity=${3:-}
keychain=${4:-}
packaging=$(cd "$(dirname "$0")" && pwd)

venv=$(mktemp -d)
trap 'rm -rf "$venv"' EXIT
python3 -m venv "$venv"
"$venv/bin/pip" install --quiet --disable-pip-version-check --require-hashes --only-binary :all: -r "$packaging/dmg-requirements.txt"

rm -f "$dmg"
"$venv/bin/dmgbuild" -s "$packaging/dmg.py" -D app="$app" "$(basename "$app" .app)" "$dmg"

if [[ -n $identity ]]; then
    flags=(--force --timestamp --sign "$identity")
    if [[ -n $keychain ]]; then
        flags+=(--keychain "$keychain")
    fi
    codesign "${flags[@]}" "$dmg"
    codesign --verify --strict --verbose=2 "$dmg"
fi
