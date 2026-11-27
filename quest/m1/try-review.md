# [XS] Try the REVIEW page

## Goal

The maintainer tries the REVIEW page on their own machine and either signs it
off or files what they find as quests: what the epic's
[What a person must check](/quest/m0/ux/README.md) asks of it, which no
headless run exercises.

## Plan

The REVIEW page is on `main` (`cargo run -p moq-studio --features capture`).
What to try, on a show going live to a relay:

- **Feel:** scrubbing the strip's head and dragging the two amber handles to
  pick a stretch, then Replay and Save clip; whether the strip keeps up.
- **Look:** whether the page reads like its
  [mock-up](/quest/m0/ux/mock/review-review.webp).
- **Readability:** the strip's ruler and AIR/AGO labels, and the cards'
  length, size, and age, at a normal window size.

An agent then turns each finding into a quest, or an edit to the quest that
owns that part of the page, and deletes this one.

## Waiting

- Maintainer: try the REVIEW page as the Plan lists and report what to change,
  or that nothing needs to. An agent cannot judge feel or taste; a kittest run
  and `just shots` already cover the clicks and the layout. Check: a comment
  or quest records the result.
