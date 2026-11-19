#!/usr/bin/env bash
# Fetch the studio's bundled faces and icons into rs/moq-studio/assets: the
# fonts from Google Fonts, cut down to Latin, Greek, and Cyrillic, and the
# Lucide icons the kit names (rs/moq-studio/src/kit/icon.rs), white so egui
# can tint them. Pass icon names to add them beside the ones already there.
#
#   nix shell nixpkgs#python3Packages.fonttools --command sh/kit-assets.sh [icon ...]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
assets="$root/rs/moq-studio/assets"
fonts_rev=cc39c4c2c304bc368c9b7c311ea2bcc6013299fe
lucide=1.73.0

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

ofl="https://raw.githubusercontent.com/google/fonts/$fonts_rev/ofl"
mkdir -p "$assets/fonts" "$assets/icons"
curl -sfL -o "$work/inter.ttf" "$ofl/inter/Inter%5Bopsz,wght%5D.ttf"
curl -sfL -o "$work/mono.ttf" "$ofl/jetbrainsmono/JetBrainsMono%5Bwght%5D.ttf"
curl -sfL -o "$work/display-semibold.ttf" "$ofl/barlowcondensed/BarlowCondensed-SemiBold.ttf"
curl -sfL -o "$work/display-bold.ttf" "$ofl/barlowcondensed/BarlowCondensed-Bold.ttf"
for family in inter jetbrainsmono barlowcondensed; do
    curl -sfL -o "$assets/fonts/$family.OFL.txt" "$ofl/$family/OFL.txt"
done

# Inter keeps its weight axis, which egui sets per family; its optical size
# is fixed at text size. The monospace is only ever medium.
fonttools varLib.instancer -q "$work/inter.ttf" opsz=14 -o "$work/inter-text.ttf"
fonttools varLib.instancer -q "$work/mono.ttf" wght=500 -o "$work/mono-medium.ttf"
unicodes="U+0000-024F,U+0300-036F,U+0370-03FF,U+0400-04FF,U+1E00-1EFF,U+2000-206F,U+20A0-20CF,U+2100-214F,U+2190-21FF,U+2212,U+2215,U+FFFD"
subset() {
    fonttools subset "$work/$1" --unicodes="$unicodes" --no-hinting \
        --layout-features="kern,liga,calt,tnum,case,ccmp,locl,mark,mkmk" --output-file="$assets/fonts/$2"
}
subset inter-text.ttf Inter.ttf
subset mono-medium.ttf JetBrainsMono-Medium.ttf
subset display-semibold.ttf BarlowCondensed-SemiBold.ttf
subset display-bold.ttf BarlowCondensed-Bold.ttf

curl -sfL -o "$work/lucide.tgz" "https://registry.npmjs.org/lucide-static/-/lucide-static-$lucide.tgz"
tar -xzf "$work/lucide.tgz" -C "$work"
cp "$work/package/LICENSE" "$assets/icons/LICENSE"
names=("$@")
for svg in "$assets"/icons/*.svg; do
    [[ -e $svg ]] && names+=("$(basename "$svg" .svg)")
done
for name in "${names[@]}"; do
    sed -e 's/currentColor/#fff/' -e '/class="lucide/d' "$work/package/icons/$name.svg" >"$assets/icons/$name.svg"
done
