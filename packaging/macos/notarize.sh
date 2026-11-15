#!/usr/bin/env bash
# Notarize a signed app bundle or disk image and staple its ticket to it, so a
# first launch offline passes Gatekeeper too. Reads the App Store Connect API
# key from APPLE_API_KEY (its ID), APPLE_API_ISSUER, and APPLE_API_PRIVATE_KEY
# (the .p8 text).
set -euo pipefail

target=$1

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
key=$work/notary.p8
printf '%s\n' "$APPLE_API_PRIVATE_KEY" >"$key"
notary=(--key "$key" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER")

# The notary service takes a disk image as is, and a bundle zipped.
submission=$target
if [[ $target == *.app ]]; then
    submission=$work/notarize.zip
    ditto -c -k --keepParent "$target" "$submission"
fi

result=$(xcrun notarytool submit "$submission" "${notary[@]}" --wait --timeout 1h --output-format json)
echo "$result"
# The log lists every issue, warnings included, even when accepted.
xcrun notarytool log "$(jq -r .id <<<"$result")" "${notary[@]}"
[[ $(jq -r .status <<<"$result") == Accepted ]]
xcrun stapler staple "$target"
xcrun stapler validate "$target"
