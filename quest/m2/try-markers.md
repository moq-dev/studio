# [XS] Try markers

## Goal

The maintainer tries markers on their own machine and either signs them off
or files what they find as quests.

## Plan

Markers are on `main` (`cargo run -p moq-studio --features capture`). What
to try, on a show going live to a relay:

- **Feel:** dropping a marker with the flag beside the panic key and with a
  bound key while a game has focus, then naming it, clipping around it, and
  removing it on REVIEW.
- **Look:** the numbered flags on REVIEW's strip and the list over the
  recordings, at a normal window size.
- **Fit:** whether 10 seconds before a marker and 5 after is the clip you
  want, and whether a marker older than 30 seconds should clip at all before
  [clips from a recording](/quest/m3/marker-clips.md) land.

An agent then turns each finding into a quest, or an edit to the quest that
owns that part, and deletes this one.

## Waiting

- Maintainer: try markers as the Plan lists and report what to change, or
  that nothing needs to. An agent cannot judge feel or taste; a kittest run
  and `just shots` already cover the clicks and the layout. Check: a comment
  or quest records the result.
