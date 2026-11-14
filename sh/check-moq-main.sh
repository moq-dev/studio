#!/usr/bin/env bash
# Fail unless a moq commit (default: the gitlink) is on moq-dev/moq's main, as
# AGENTS.md requires of the pin. It fetches main, so `just check` doesn't run it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
moq="$root/moq"

if [ $# -gt 0 ]; then
    rev="$1"
else
    rev="$(git -C "$root" ls-tree HEAD moq --object-only)"
fi

# A shallow checkout, such as CI's, lacks main's history: fetch every commit
# but no trees or blobs, since the ancestry test reads only commits.
if [ "$(git -C "$moq" rev-parse --is-shallow-repository)" = true ]; then
    git -C "$moq" fetch --quiet --no-tags --unshallow --filter=tree:0 origin main
else
    git -C "$moq" fetch --quiet origin main
fi

if ! commit="$(git -C "$moq" rev-parse --quiet --verify "$rev^{commit}")"; then
    echo "check-moq-main: '$rev' is not a commit in moq/ or on moq's main" >&2
    exit 1
fi

# Exit 1 means "not an ancestor"; anything else is git failing.
status=0
git -C "$moq" merge-base --is-ancestor "$commit" FETCH_HEAD || status=$?
case "$status" in
    0) ;;
    1)
        echo "check-moq-main: moq ${commit:0:12} is not on moq-dev/moq's main -> land it in moq first, then 'just upgrade-moq'" >&2
        exit 1
        ;;
    *) exit "$status" ;;
esac
