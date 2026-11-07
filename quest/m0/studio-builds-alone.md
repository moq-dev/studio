# [XS] moq-studio builds on its own

## Goal

`cargo build -p moq-studio` succeeds on its own, not only inside a workspace
build, because every feature the studio uses is enabled by its own
dependencies.

## Plan

From [moq-dev/studio#84](https://github.com/moq-dev/studio/pull/84), picked
2026-11-07 (maintainer away under /afk, recommended option), m0 by the rule
that fixes join m0.

`rs/moq-studio/src/health.rs` awaits moq-stats' async wrappers, but only a
test dependency turns on moq-stats' `async` feature, so the workspace build
and CI unify it in and never notice. Enable `async` on the studio's own
moq-stats dependency, and check the other moq crates the studio awaits for
the same gap. Recommendation, not settled: have `just check` build
`moq-studio` on its own so CI catches the next one.
