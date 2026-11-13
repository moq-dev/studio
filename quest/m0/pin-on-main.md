# [XS] The moq pin is checked to be on moq's main

## Goal

A PR that points the `moq` submodule at a commit not on moq-dev/moq's `main`
fails a check, as `AGENTS.md:14` already requires in prose.

## Plan

From studio's trimmed Pullfrog learnings (2026-11-12, maintainer,
/quest-plan).

`sh/check-moq-pin.sh` checks only that the checkout matches the gitlink, that
the submodule is clean, and that no moq git dependency appears beside it.
`sh/upgrade-moq.sh` fetches moq's `main` but checks out any revision given.

- Add a script that fetches moq's `main` into the submodule with its whole
  commit history, since CI's submodule checkout is shallow (`--unshallow`
  where it is, with `--filter=tree:0` to keep it small), and fails unless
  `git merge-base --is-ancestor <gitlink> FETCH_HEAD` holds.
- Run it as its own step in `.github/workflows/check.yml`, not from
  `just check`, so a local `just check` stays offline, and only on a pull
  request whose `moq` gitlink differs from its base's
  (`github.event.pull_request.base.sha`), so an unchanged pin fetches
  nothing and never fails on a network error.
- `just upgrade-moq <rev>` runs the same ancestry test after its fetch and
  refuses a revision off `main`.

[pin-main](/quest/m0/pin-main.md) catches the studio up to moq's `main` and
is unaffected: the current pin, `644d48996`, is on `main`.
