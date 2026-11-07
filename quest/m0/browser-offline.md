# [S] Browser sources make no outside requests

## Goal

The CEF browser helper contacts only what its pages load: no component
updates, spellcheck dictionary downloads, network time, or other background
requests to Google. The browser suite asserts that no outside request is
made.

## Plan

From [moq-dev/studio#78](https://github.com/moq-dev/studio/pull/78), picked
2026-11-07 (maintainer away under /afk, recommended option), m0 by the rule
that fixes join m0: each request is a privacy leak and a network dependency
nobody asked for.

Seen from the helper: the component updater (`update.googleapis.com`,
`edgedl.me.gvt1.com`), the spellcheck dictionary (`redirector.gvt1.com`),
network time (`clients2.google.com`), and `www.google.com/async/folae`.
Chromium's switches, such as `--disable-component-update` and
`--disable-background-networking`, and turning spellcheck off should stop
them; confirm each one is gone rather than assuming. Pages run without
Chromium's sandbox on Linux (decided on #78), so keep the helper's switches
in one place.
