# [L] Toolkit spike for the redesign

## Goal

The LIVE page of the [mock-up](/quest/m0/ux/mock/shell-live.webp) runs twice,
once on egui and once on [Rune](https://github.com/anomalyco/rune): the title
bar with its pages, health dot, and live control, the live picture in its
tally frame, the scene pads, and the sound strip, on the real compositor.
Its PR reports what each cost and recommends one, and the maintainer picks the
toolkit every other quest in this epic builds on.

## Plan

Decided 2026-11-09 (maintainer, /quest-plan): ✅ spike both before any page is
built (rejected: staying on egui unasked; moving to Rune unasked). The Rune
spike of 2026-11-07,
[moq-dev/studio#98](https://github.com/moq-dev/studio/pull/98), said stay on
egui. Its branch `spike/rune-ui` and `sh/rune-spike/rune.patch` are the
starting point for the Rune half. The redesign has no dock, which was one of
Rune's costs there; the others stand until this spike says otherwise: Rune
cannot draw on the studio's own wgpu device without a patch, takes no pull
requests, and has no release.

Both halves stay off `main`, as #98's did. The PR deletes this quest and links
both branches.

The report compares, on the same scene and host:

- **Fidelity:** each half's screenshot beside the mock-up frame, and what
  could not be matched. Count what each toolkit lacks and the studio would
  own: the pads, the segmented meters, popovers and tooltips, a custom title
  bar with the window's drag region and controls, text in the three faces.
- **Frame time**, measured as AGENTS.md requires (a real compositor, not
  Xvfb), and that the program's render time is unchanged.
- **Windows:** a second native window sharing the compositor's textures,
  which [multiview](/quest/m1/multiview.md), the
  [mini controller](/quest/m1/mini-controller.md), and
  [panels](/quest/m1/panels.md) need, and an always-on-top borderless one.
- **Testing:** what a headless run can assert and screenshot, which decides
  how [screenshots](/quest/m0/ux/shots.md) work.
- **Risk:** what each asks of upstream, and what a year of maintenance looks
  like. The web build no longer weighs on this: it moved to m2 on 2026-11-09.

No fixed pass bar: the maintainer decides from the report. Hand the PR back
with that decision under `## Waiting`, and record the answer in
[the epic](/quest/m0/ux/README.md) and m0's README, whose 2026-10-22 entry
chose egui with egui_dock.

## Related

- [The look](/quest/m0/ux/kit.md) - the first quest the choice unblocks
