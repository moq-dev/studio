# [S] The program reaches a relay on every runner

## Goal

`engine::tests::the_program_reaches_a_relay` passes every time on GitHub's
Ubuntu runner, as it does on the maintainer's hosts, so it never fails the
nightly before the browser suite runs.

## Plan

From [moq-dev/studio#78](https://github.com/moq-dev/studio/pull/78), picked
2026-11-07 (maintainer away under /afk, recommended option), m0 by the rule
that fixes join m0.

In the nightly's Software render job (run 42350943682, attempt 1), the test
failed with `Stream(NotFound)` before the browser suite ran. It passed 30 of
30 times on nixos2. Reproduce it, for example under CPU contention or on a
fresh runner, find which stream is not found and why (a subscribe racing the
announce is a first suspect), and fix the cause. Never add a retry, sleep,
or timeout.
