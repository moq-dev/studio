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
# The join page's workspace includes moq's JS packages, whose dependencies move too.
(cd "$root" && bun install)
echo "moq -> $(git -C "$root/moq" log --oneline -1)"
