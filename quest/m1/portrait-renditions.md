# [M] The portrait picture is renditions of the program

## Goal

With **Both at once** on, the program broadcast carries its portrait
renditions beside the landscape ones, in one catalog with one sound, and
`<broadcast>/portrait` is no longer published. A phone viewer in portrait gets
the portrait picture from the same link a desktop viewer opens.

## Plan

Decided 2026-11-20 (maintainer, /quest-plan): ✅ portrait and landscape are
renditions of one broadcast, which players tell apart by coded size
(rejected: a catalog field naming a sibling portrait broadcast; keeping the
`<broadcast>/portrait` path). ✅ The show's main format ranks first in the
catalog (`VideoConfig::preference`), so a consumer with no viewport, such as a
recorder or a restream, takes it (rejected: always landscape).

[moq-dev/studio#200](https://github.com/moq-dev/studio/pull/200) built the
portrait picture as its own broadcast, with the same audio scene as the
program. The composition keeps placing landscape only and lists only the
landscape renditions. With the path gone, `moq_scene::PORTRAIT` and the rule
refusing a published scene named `portrait` (`rs/moq-scene/src/format.rs`) go
too. Update [the dual-format check](/quest/m1/dual-format-check.md),
which watches `<broadcast>/portrait` on a phone, in this PR.

`VideoConfig::preference` is a decode tier today: `@moq/watch` keeps only a
catalog's highest preference before it selects (`preferredRenditions` in
`js/watch/src/video/source.ts`), so a lower-ranked orientation stays reachable
only once the orientation pick decided below filters by orientation before that
tier.

Decided 2026-11-26 (maintainer, /quest-run): ✅ the studio reads `@moq/watch`
from the `moq` submodule (`workspace:*`, as `js/join` does), so it waits on the
pin carrying
[the orientation quest](https://github.com/moq-dev/moq/pull/9287) (landed
2026-11-21), not on an npm release (rejected: waiting on npm's `@moq/watch`).
A player on an older `@moq/watch` keeps only the top preference, the show's
main format, so it never lands on portrait unless that is the main format.

## Required

- [The studio catches up to moq's main](/quest/m0/pin-moq-tip.md) - its pin carries the players' orientation pick
