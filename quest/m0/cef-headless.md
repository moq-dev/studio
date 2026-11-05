# [S] Browser sources without an X display

## Goal

Browser sources run with no X server or `$DISPLAY` on both the CPU path and
the GPU (shared DMA-BUF texture) path, so the studio's CEF helper works under a
pure Wayland session without XWayland and `just render-sw` no longer needs
`xvfb-run`. Copy and paste under Interact still reach the desktop clipboard.

## Plan

From [moq-dev/studio#49](https://github.com/moq-dev/studio/pull/49), picked
2026-11-05 (maintainer, /quest-plan), m0 by the rule that fixes join m0. With
`--disable-gpu` alone CEF still fails with "Missing X server or $DISPLAY"; a
trial adding `--ozone-platform=headless` on the CPU path passed 10/10 runs
with no display at about 0.4 s, and the browser suite matched Xvfb.

Check first, and record the result in the PR:
- Whether Copy and Cut under Interact still reach the desktop clipboard with
  the headless platform; if not, the studio's own clipboard (egui) relays
  them.
- Whether the GPU path can go headless too. If it cannot, land the CPU path
  and rewrite this quest to the GPU path's remaining work, with what blocks
  it.

Then drop the `xvfb-run` wrapper from the `render-sw` recipe in the
`justfile` (the nightly job inherits it through the recipe), and say in
`README.md` that browser sources need no X server.

## Required

- [Deterministic browser source startup](/quest/m0/cef-startup.md) - the `--disable-gpu` CPU path this builds on
