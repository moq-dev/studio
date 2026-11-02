#!/usr/bin/env bash
# The moq crates are path dependencies into the `moq` submodule, so its gitlink
# is the single pin. Fail if the checkout drifts from it, the submodule is
# dirty, or a moq git dependency appears beside it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fail=0

gitlink="$(git -C "$root" ls-tree HEAD moq --object-only 2>/dev/null || true)"
if [ -z "$gitlink" ]; then
    echo "check-moq-pin: no 'moq' submodule recorded at the repo root" >&2
    exit 1
fi

case "$(git -C "$root" submodule status -- moq 2>/dev/null || echo '?')" in
    " "*) ;;
    "-"*)
        echo "check-moq-pin: moq/ is not initialized -> run 'git submodule update --init'" >&2
        fail=1
        ;;
    "+"*)
        echo "check-moq-pin: moq/ is checked out at a different commit than the gitlink ${gitlink:0:12}" >&2
        echo "  -> commit the gitlink with Cargo.lock, or 'git submodule update' to go back" >&2
        fail=1
        ;;
    *)
        echo "check-moq-pin: could not read 'git submodule status moq'" >&2
        fail=1
        ;;
esac

# Cargo compiles the submodule's working tree, untracked build scripts included,
# so a dirty one builds something no committed checkout reproduces.
dirty="$(git -C "$root/moq" status --porcelain --untracked-files=all 2>/dev/null || true)"
if [ -n "$dirty" ]; then
    echo "check-moq-pin: moq/ has uncommitted changes; the build would not be reproducible" >&2
    fail=1
fi

if grep -qE '^[^#]*moq-dev/moq' "$root/Cargo.toml" || grep -q 'github.com/moq-dev/moq' "$root/Cargo.lock"; then
    echo "check-moq-pin: a moq git dependency is declared; the submodule is the pin -> use 'path = \"moq/rs/<crate>\"'" >&2
    fail=1
fi

exit "$fail"
