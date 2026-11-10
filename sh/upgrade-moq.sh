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

# Print the first registry crate both locks hold at semver-compatible but
# different versions, as `<name>@<studio version> <moq version>`.
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
            else if (key in moq && moq[key] != version) { print name "@" version, moq[key]; exit }
        }
    ' "$root/moq/Cargo.lock" "$root/Cargo.lock"
}

# Cargo resolves a crate the new moq adds at its newest release, not the one
# moq's lock tested, and that release can pull a second copy of a crate moq
# holds once: two copies of rind's assembly fail the fat-LTO release link.
# So move every shared crate to moq's version; one the studio needs newer fails here.
last=""
while next="$(drift)" && [ -n "$next" ]; do
    if [ "$next" = "$last" ]; then
        echo "upgrade-moq: cargo did not move ${next% *} to moq's ${next#* }" >&2
        exit 1
    fi
    cargo update --manifest-path "$root/Cargo.toml" --quiet -p "${next% *}" --precise "${next#* }"
    last="$next"
done
# The join page's workspace includes moq's JS packages, whose dependencies move too.
(cd "$root" && bun install)
echo "moq -> $(git -C "$root/moq" log --oneline -1)"
