# [S] A browser helper exits with the studio

## Goal

When the studio process dies for any reason (a crash, a kill, or a test that
times out), its CEF browser helper and the helper's children exit too, on
Linux, macOS, and Windows. No orphaned helper keeps running against the
desktop.

## Plan

From [moq-dev/studio#73](https://github.com/moq-dev/studio/pull/73), picked
2026-11-07 (maintainer, /quest-complete), m0 by the rule that fixes join m0.

- The helper runs in its own process group
  (`rs/moq-scene/src/browser/run.rs`), which is killed only when the studio
  drops it in an orderly way. On nixos2, three test runs that timed out left
  helpers spinning against GNOME until they were killed by hand.
- Reproduce first: kill the studio while a page is open and see what the
  helper does. Its channel to the studio closes then; find why that does not
  end it.
- Fix the cause in a way that holds on every desktop, such as the helper
  exiting when that channel closes. Linux's `PR_SET_PDEATHSIG` alone would
  leave macOS and Windows uncovered.
