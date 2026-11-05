# [S] Deterministic browser source startup

## Goal

A browser source paints its first frame within a bounded, repeatable time
when its frames come back over the CPU path, and the browser source tests
no longer take 4 to 33 seconds or hit their budget.

## Plan

From [moq-dev/studio#8](https://github.com/moq-dev/studio/pull/8).

On the CPU path (no shared GPU texture, and CI under `just render-sw`), this
build's CEF cannot load GL. Chromium walks its GPU fallback chain, and in the
slow runs a fallback GPU process fails "Failed to create shared context for
virtualization", then hangs until Chromium's GPU watchdog kills it, 4 to 11
seconds later, sometimes twice.

Decided 2026-11-04 with the maintainer (/quest-plan):

- ✅ **Pass `--disable-gpu` on the CPU path.** The frames come back on the
  CPU anyway, so software raster gives deterministic startup. WebGL pages
  run slower on that path.
  Rejected: packaging GL and EGL for CEF (keeps WebGL accelerated, but adds
  packaging and driver variance).

Also check whether CEF starts without an X display under this flag
(`--ozone-platform=headless` started before, but its GPU process failed
EGL), and record the result in the quest's PR.

Verify by running `a_click_reaches_the_page` many times under
`just render-sw`, and report the startup time's spread before and after.
