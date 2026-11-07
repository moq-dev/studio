# [M] Nightly and the browser suite pass

## Goal

The nightly workflow passes again, and the browser suite
(`rs/moq-studio/tests/browser.rs`) passes on every path it runs: GPU under
Wayland and X11, and the CPU path with no display.

## Plan

From [moq-dev/studio#73](https://github.com/moq-dev/studio/pull/73), picked
2026-11-07 (maintainer, /quest-complete), m0 by the rule that fixes join m0.

Nightly has failed every night since 2026-11-04; it last passed on
2026-11-03. Known failures, each fixed at its cause:

- `render-sw`: browser helpers exit on SIGTRAP
  ([run 42235026329](https://github.com/moq-dev/studio/actions/runs/42235026329)).
- `cookies_survive_a_restart` fails on every path, including the CPU path on
  `main`.
- `copy_and_cut_reach_the_clipboard` fails on X11: on that path a copy goes
  to the X clipboard, but the test reads only the studio's own copy relay. It
  first ran that far once #73 made X11 paint.

Check the latest nightly run first, since other failures may have joined
these.
