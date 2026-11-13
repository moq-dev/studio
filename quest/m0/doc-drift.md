# [S] Docs and test notes match the code

## Goal

Ignored tests name a recipe that exists, and `just check` keeps it so,
`record.rs` describes how recording actually encodes, and `CONTRIBUTING.md`'s
PR rules fit the quest PRs agents open, so neither people nor agents follow
instructions that fail.

## Plan

Decided 2026-11-12 (maintainer, /quest-plan), from studio's trimmed Pullfrog
learnings:

- `CONTRIBUTING.md:9` and `:15-21` require conventional-commit titles and
  Problem/Approach/Impact sections on every PR, but quest PRs (#108, #110,
  #112, #114, #115, #126, #141, #143) use the quest guide's sections and
  `quest` type titles, such as `quest:` or `quest(m0/ux):`. Which changes?
  - ✅ Exempt quest PRs: a PR that only adds or edits quests follows the quest
    guide's format and a `quest` type title, with or without a scope
    (rejected: make quest PRs carry the template too).
- 59 `#[ignore]` reasons say `just rs render-sw` (54 in
  `rs/moq-scene/src/compositor.rs`, 5 in `rs/moq-studio/src/engine.rs`), but
  the recipe is `just render-sw` in the root `justfile`. Fix every one main
  holds, and add a `just check` step that fails when an `#[ignore]` reason
  names a `just` recipe the root `justfile` does not define (`just
  --summary` lists them). Open PR
  [moq-dev/studio#146](https://github.com/moq-dev/studio/pull/146) adds
  another with the wrong name and edits the recipe. Check on a PR tests its
  merge with `main` as of its last run, so if this lands first, #146's next
  Check runs the lint, and if #146 lands first, this fixes its reason too.
  `main`'s ruleset does not require an up-to-date branch, so a Check that
  predates the lint still counts. #146 is a draft held until moq's timed
  catalog is pinned, so it cannot merge before this lands; right after this
  lands, update its branch with `main` (`gh pr update-branch 146`), or ask
  the session holding it to, so its next Check runs the lint before it can
  be marked ready. If #146 is ready for review by then, hold it from merging
  until that Check passes.
- `rs/moq-studio/src/record.rs:4-6` says a recording of the program shares
  the live encoders, but `rs/moq-studio/src/engine.rs:1811-1852` gives the
  recording its own full-size encoder (`recorded_video`, `record: true`) in
  a hidden broadcast. Rewrite only the program's half: the sources' half
  (`record.rs:6-9`) is accurate, since local sources are read from the
  studio's local broadcast (`engine.rs:932-935`).
