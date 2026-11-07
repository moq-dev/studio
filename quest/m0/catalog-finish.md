# [S] Catalogs finish when the studio quits

## Goal

When the studio quits or stops publishing, the program's catalog and the
recording's catalog end with `finish()`, so a viewer reading either sees a
clean end, not a cut-off track, and the studio logs no
`track::Producer dropped without finish() or abort()` warning on quit.

## Plan

From [moq-dev/studio#91](https://github.com/moq-dev/studio/pull/91), picked
2026-11-07 (maintainer away under /afk, recommended option), m0 by the rule
that fixes join m0.

On quit the studio logs that warning for `catalog.json` and `catalog`, twice.
`Program::drop` (`rs/moq-studio/src/engine.rs`) closes the program's broadcast
without finishing its catalog, and the recording's catalog is dropped the same
way. First confirm from the viewer's side what a viewer reading the catalog
sees today, then finish each catalog before its broadcast closes. A test
reads the program's catalog through a relay, quits, and sees it finish.
