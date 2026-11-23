# [XS] moq-video reports where each display sits

## Goal

The `moq` submodule includes moq-video reporting each listed display's
rectangle on the desktop and its scale, and, for a display picked through
the ScreenCast portal, the position the portal reports for its stream.

## Plan

Planned 2026-11-22 (maintainer, /quest-plan) for
[the desktop frame](/quest/m1/desktop-frame.md), which draws over the exact
desktop pixels a capture holds. Today `Display` has an id, a name, and a
size, and moq's [capture listing](https://github.com/moq-dev/moq/blob/main/quest/m3/capture-list.md)
plans a window's rectangle on its display but not the display's place on
the desktop. Advance it with `just upgrade-moq` in its own PR.

## Waiting

- moq-dev/moq: a quest for a display's rectangle on the desktop and its scale, and a portal-picked monitor's position, is planned (run `/quest-plan` in moq-dev/moq; it may extend the capture listing quest) and lands on moq's `main`. Check its PR has merged.
