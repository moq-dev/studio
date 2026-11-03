# [S] Outside edits to the scene file

## Goal

Autosave never silently overwrites a change made to the scene file outside the
studio, by hand or by another studio on the same file.
When the studio has nothing unsaved, it reloads the outside change without
asking.
When both sides changed, autosave pauses and the studio asks whether to keep
its own version or take the file's; nothing is written until the user picks.

## Plan

Decided 2026-11-02 (maintainer, follow-up planning after
[#4](https://github.com/moq-dev/studio/pull/4)): the silent reload and the
keep-mine or take-theirs prompt above. Today the last writer wins, which
`README.md` documents; that note goes away with this quest.

- Detect a change by what the studio last wrote or read, not by timing, so the
  studio's own atomic rename never reads as an outside edit.
- Check again right before the autosave's rename, not only before its write,
  since another studio can write in between. A window remains between that
  check and the rename; close it with an advisory lock between studios if one
  works on every platform, or else name the residual race in `README.md`.
- An outside edit that does not parse is refused like File > Reload refuses
  it: the studio keeps its state, says why, and does not overwrite the file
  until the user picks.
- The undo history file follows the scene file: a reload already replays or
  drops its entries, and "take theirs" goes through that path.
