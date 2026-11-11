# [S] Windows installer

## Goal

Every release ships an unsigned Windows installer of `moq-studio`, built in
CI with the `release` feature and verified to launch and list devices on
Windows.

## Plan

The feature policy is the [epic](/quest/m0/packaging/README.md)'s.

The installer's and executable's icon is an `.ico` that the build generates
from `packaging/live.kixel.studio.svg`, the only drawn copy; never check the
`.ico` in.

Decided 2026-11-06 (maintainer, /quest-unblock): Windows ships unsigned
until a release needs SmartScreen reputation; see
[signing](/quest/m0/signing.md).

## Required

- [Studio on Windows](/quest/m0/app-windows.md) - the studio running on Windows
