#!/usr/bin/env bash
# An `#[ignore]` reason tells the reader which `just` recipe runs the test, so
# fail if one names a recipe the root justfile does not define.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
read -ra recipes <<<"$(just --justfile "$root/justfile" --summary)"
fail=0

# Tracked files only, so the moq submodule, whose recipes are its own, is skipped.
while IFS= read -r match; do
    reason=$(grep -oE 'ignore = "[^"]*"' <<<"$match")
    while read -r _ recipe; do
        [[ " ${recipes[*]} " == *" $recipe "* ]] && continue
        echo "check-ignore-recipes: ${match%%:*}: the reason names 'just $recipe', which the justfile does not define" >&2
        fail=1
    done < <(grep -oE '(^|[^[:alnum:]_])just [[:alnum:]_-]+' <<<"$reason" | grep -oE 'just [[:alnum:]_-]+')
done < <(git -C "$root" grep -nE 'ignore = "[^"]*just ' -- '*.rs' || true)

exit "$fail"
