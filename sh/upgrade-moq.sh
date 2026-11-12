#!/usr/bin/env bash
# Move the moq submodule to a moq commit (default: origin/main) and refresh
# Cargo.lock and bun.lock to match. Commit the gitlink and both locks together.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
rev="${1:-origin/main}"

git -C "$root/moq" fetch --quiet origin main
git -C "$root/moq" checkout --quiet --detach "$rev"
# Resolve without --locked: Cargo adds and prunes only what the new moq needs.
cargo metadata --manifest-path "$root/Cargo.toml" --format-version 1 >/dev/null

# Print each registry crate both locks hold at semver-compatible but different
# versions, as `<name>@<studio version> <moq version>`.
drift() {
    awk '
        function compat(v, p) {
            split(v, p, ".")
            return p[1] != "0" ? p[1] : p[2] != "0" ? "0." p[2] : "0.0." p[3]
        }
        FNR == 1 { file++ }
        /^name = / { name = $3; gsub(/"/, "", name) }
        /^version = / { version = $3; gsub(/"/, "", version) }
        /^source = "registry/ {
            key = name " " compat(version)
            if (file == 1) moq[key] = version
            else if (key in moq && moq[key] != version) print name "@" version, moq[key]
        }
    ' "$root/moq/Cargo.lock" "$root/Cargo.lock"
}

# Cargo resolves a crate the new moq adds at its newest release, not the one
# moq's lock tested, and that release can pull a second copy of a crate moq
# holds once: two copies of rind's assembly fail the fat-LTO release link.
# So move every shared crate to moq's version. A crate a newer dependent holds
# back (rind under a newer rind-rcgen) moves on a later pass, once that
# dependent has moved; a pass that moves nothing means the studio needs one
# newer, and fails here.
while pending="$(drift)" && [ -n "$pending" ]; do
    moved=""
    while read -r crate version; do
        if cargo update --manifest-path "$root/Cargo.toml" --quiet -p "$crate" --precise "$version" 2>/dev/null; then
            moved=1
        fi
    done <<<"$pending"
    if [ -z "$moved" ]; then
        echo "upgrade-moq: cargo could not move these to moq's versions:" >&2
        echo "$pending" >&2
        exit 1
    fi
done
# The join page's workspace includes moq's JS packages, whose dependencies move too.
(cd "$root" && bun install)
echo "moq -> $(git -C "$root/moq" log --oneline -1)"
