# [S] Studio lands through Kixel Quest's queue

## Goal

`main` lands through Kixel Quest's own queue. A reviewed pick that is only
behind `main` is built squashed onto the current `main` and landed, so no PR
merges `main` because another PR landed first. An admin can still merge past
the queue.

Only studio's side is in scope. Moving other moq-dev repositories is not.

## Plan

Decided 2026-11-25 (maintainer, `/quest-run`):

- ✅ Kixel Quest's own queue (rejected: GitHub's merge queue, which this
  public repository could use today; accepting the churn). Its queue is
  [kixel-ai/kixel.quest `quest/m0/self-queue.md`](https://github.com/kixel-ai/kixel.quest/blob/main/quest/m0/self-queue.md),
  planned 2026-11-24 for the same cost there.
- ✅ The goal above, with studio's side only (rejected: moving every moq-dev
  repository, which dotfiles' tree would own).
- m0 (agent's choice): every `/quest-run` here pays the cost, and m0 is where
  that work happens.

Decided 2026-11-27 (maintainer, CI runner policy): CI runs no checks on pull
requests, so Check runs only on pushes to `main` and the queue is the only
synchronous gate. Check on the queue's candidate is the gate, and the ruleset
requires no context a PR head would have to report.

Why: `main` has no merge queue and no `quest land` check, so `quest merge`
takes its ungated path. There it refuses a head that
does not contain `main`. In the 2026-11-25 run, each landing put every other
pick behind again: about ten base merges, each with an agent turn and a CI run,
across moq-dev/studio#251, #252, #254, #256 and #257, none of which conflicted.

In this order, so the ruleset never requires a check nothing runs:

1. Once the kixel.quest item clears, land a PR in which Check
   (`.github/workflows/check.yml`) builds the queue's candidate pushes on the
   branch self-queue names (`quest-queue/main/candidate` as planned). Confirm
   a push to that branch reports Check. That PR rewrites this quest to the
   steps left.
2. Only then does the dotfiles item below apply the ruleset.
3. Validate on this repository: two reviewed picks behind `main` land in one
   candidate with no base merge, and `main`'s tip is the tested candidate; a
   pick that breaks Check in the candidate is dropped with the check named.
   Update `AGENTS.md`'s Development section if landing changes what an agent
   does before pushing.

## Waiting

- kixel-ai/kixel.quest: `quest/m0/self-queue.md` lands, and its queue serves a
  repository. Check: that file is gone from kixel.quest's `main` with its PR
  merged, and its README says how another repository joins.
- kixel-ai/dotfiles: `bin/repo-settings` gives moq-dev/studio's `main` the
  queue's ruleset, requiring `quest land` with the kixel-quest App as an
  `always` bypass actor. Plan it there with `/quest-plan` once step 1
  is on `main` and kixel.quest's own ruleset sets the pattern.
  Check: `repo-settings check` passes and `gh api
  repos/moq-dev/studio/rules/branches/main` lists `quest land`.
