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
landscape renditions. Update [the dual-format check](/quest/m1/dual-format-check.md),
which watches `<broadcast>/portrait` on a phone, in this PR.

## Waiting

- moq-dev/moq: `@moq/watch` and `moq play` pick a rendition by the viewport's
  orientation, as
  [Players pick a rendition by the viewport's orientation](https://github.com/moq-dev/moq/blob/main/quest/m1/orientation.md)
  plans (being planned in [moq-dev/moq#9247](https://github.com/moq-dev/moq/pull/9247)),
  and a `@moq/watch` release carries it, so a desktop viewer never lands on
  portrait. Check: that quest's PR has merged on moq's `main`, and npm's
  newest `@moq/watch` is past it.
