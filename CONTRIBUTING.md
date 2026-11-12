# Getting oriented

Start with the [README](README.md) for what the studio does and how to build it.

# Commits

PRs into `main` are squash-merged, so the PR title becomes the commit subject in `git log`.

- Use conventional-commit subjects (`feat(scene): ...`, `fix: ...`, `chore: ...`, `docs: ...`).
- Never commit binaries or build artifacts. Keep other binary files, such as images and fixtures, under 500 KiB.

# PRs

Keep the body short and structured, not narrated.
Have at least these sections:

- **Problem**: a summary of the problem and why this PR is needed.
- **Approach**: a summary of the approach taken to solve the problem.
- **Impact**: a bullet point for every user-visible change (flags, the scene file format, outputs). A breaking one says how to migrate, and the PR title marks it with `!` (`feat(studio)!: ...`).
- **Alternatives**: any alternative approaches considered.
- **Follow-ups**: any issues encountered or quests created.

# Landing

Ready PRs into `main` run **Check** (`just ci` on a hosted runner); drafts skip it.
Queue the reviewed head with `/quest-merge` once the maintainer has approved merging that PR.

# Bumping moq

The `moq` submodule pins moq-dev/moq.
`just upgrade-moq` moves it to moq's latest `main` and refreshes `Cargo.lock`; land the bump as its own PR, fixing whatever the new moq API breaks in the same PR.
