# [XS] The moq pin is checked to be on moq's main

## Goal

A PR that points the `moq` submodule at a commit not on moq-dev/moq's `main`
fails a check, as `AGENTS.md:14` already requires in prose.

## Plan

Decided 2026-11-12 (user, /quest-plan), from studio's trimmed Pullfrog
learnings.

`sh/check-moq-pin.sh` checks only that the checkout matches the gitlink, that
the submodule is clean, and that no moq git dependency appears beside it.
`sh/upgrade-moq.sh` fetches moq's `main` but checks out any revision given.

Recommended: in `sh/check-moq-pin.sh`, fetch moq's `main` and fail unless
`git merge-base --is-ancestor <gitlink> origin/main` holds; run it where a
fetch is allowed, and make `just upgrade-moq <rev>` refuse a revision off
`main` the same way. [pin-main](/quest/m0/pin-main.md) catches the studio up
to moq's `main` and is unaffected.
