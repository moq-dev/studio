# [S] Docs and test notes match the code

## Goal

Ignored tests name a recipe that exists, `record.rs` describes how recording
actually encodes, and `CONTRIBUTING.md`'s PR rules fit the quest PRs agents
open, so neither people nor agents follow instructions that fail.

## Plan

Decided 2026-11-12 (user, /quest-plan), from studio's trimmed Pullfrog
learnings:

- `CONTRIBUTING.md:9` and `:15-21` require conventional-commit titles and
  Problem/Approach/Impact sections on every PR, but quest PRs (#108, #110,
  #112, #114, #115, #126, #141, #143) use the quest guide's sections and
  `quest:` titles. Which changes?
  - ✅ Exempt quest PRs: a PR that only adds or edits quests follows the quest
    guide's format and a `quest:` title (rejected: make quest PRs carry the
    template too).
- 59 `#[ignore]` reasons say `just rs render-sw` (54 in
  `rs/moq-scene/src/compositor.rs`, 5 in `rs/moq-studio/src/engine.rs`), but
  the recipe is `just render-sw` in the root `justfile`. Open PR
  [moq-dev/studio#146](https://github.com/moq-dev/studio/pull/146) adds
  another with the wrong name and edits the recipe; fix whatever main holds
  when the work starts.
- `rs/moq-studio/src/record.rs:4-6` says a recording shares the live
  encoders, but `rs/moq-studio/src/engine.rs:1806-1810` gives each recording
  its own full-size encoder in a private broadcast. Make the comment match
  the engine.
