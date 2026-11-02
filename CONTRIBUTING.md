# Getting oriented

Start with the [README](README.md) for what the studio does and how to build it.
Agents should also read [AGENTS.md](AGENTS.md) and the instructions nested under the files they change.

# Commits

PRs into `main` are squash-merged, so the PR title becomes the commit subject in `git log`.

- Use conventional-commit subjects (`feat(scene): ...`, `fix: ...`, `chore: ...`, `docs: ...`).
- AI commit attribution goes in a `Co-Authored-By:` trailer, not the commit body.
- Never commit binaries or build artifacts. Keep other binary files, such as images and fixtures, under 500 KiB.

# PRs

Keep the body short and structured, not narrated.
Have at least these sections:

- **Problem**: a summary of the problem and why this PR is needed.
- **Approach**: a summary of the approach taken to solve the problem.
- **Impact**: a bullet point for every user-visible change (flags, the scene file format, outputs). A breaking one says how to migrate, and the PR title marks it with `!` (`feat(studio)!: ...`).
- **Alternatives**: any alternative approaches considered.
- **Follow-ups**: any issues encountered or quests created.

When pushing additional commits to an existing PR, update the title and description if needed.
Never rebase or force-push a pushed branch; merge the base branch in instead.
Check that the PR is still open before pushing to its branch, since a push after merge recreates the deleted branch.

Open a draft PR while working, without asking once the scope is settled.
Run `just ci` before every push, and mark the PR ready yourself when the work is done.
Keep it a draft only while its description lists a decision the maintainer still owns.

# Landing

Ready PRs into `main` run **Check** (`just ci` on a hosted runner); drafts skip it.
Queue the reviewed head with `/quest-merge` once the maintainer has approved merging that PR.

# Bumping moq

The `moq` submodule pins moq-dev/moq.
`just upgrade-moq` moves it to moq's latest `main` and refreshes `Cargo.lock`; land the bump as its own PR, fixing whatever the new moq API breaks in the same PR.

# Reviews

Before marking a PR ready for review, and before each push to a ready PR, run `/quest-review` on the committed branch after the local checks pass.
It uses `quest review` to have the other vendor's model review against the PR's base branch, with `REVIEW.md` read from that base.

The PR body's `## Review` section records the reviewer harness, model and effort, reviewed head SHA, and each finding's disposition, or a skip and its reason when the author judges the change too trivial to review.
Address online reviewers' findings already posted on the head when merging; never wait for or request an online review.

# Follow-ups

Record out-of-scope findings as follow-up quests with `/quest-plan`, not GitHub issues, which are for outside contributors.
