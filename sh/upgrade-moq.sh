#!/usr/bin/env bash
# Move the moq submodule to a moq commit (default: origin/main) and refresh
# Cargo.lock to match. Commit the gitlink and Cargo.lock together.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
rev="${1:-origin/main}"

git -C "$root/moq" fetch --quiet origin main
git -C "$root/moq" checkout --quiet --detach "$rev"
# Resolve without --locked: Cargo adds and prunes only what the new moq needs.
cargo metadata --manifest-path "$root/Cargo.toml" --format-version 1 >/dev/null
echo "moq -> $(git -C "$root/moq" log --oneline -1)"
