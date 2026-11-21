# [M] Chromium's sandbox for browser sources on Windows

## Goal

A browser source's renderer on Windows runs inside Chromium's sandbox, as it
does on Linux and macOS, so a hostile page, or an overlay site that gets
compromised, cannot read the creator's files, keys, or stream credentials
through a renderer exploit. The helper passes no `--no-sandbox`.

## Plan

Linux and macOS landed in
[moq-dev/studio#223](https://github.com/moq-dev/studio/pull/223): on Linux
`moq_scene::browser::sandbox::check` refuses to start CEF where the kernel
refuses the user namespaces Chromium needs, and on macOS each helper app
enters the Seatbelt sandbox through CEF's `libcef_sandbox.dylib`
(`cef::sandbox::Sandbox`) before it loads CEF's framework.

- CEF's Windows sandbox needs `cef_sandbox.lib` linked into the executable
  that runs the subprocesses, with its sandbox info passed to
  `execute_process` and `initialize`, and `bootstrap.exe`'s layout. The `cef`
  crate's `sandbox` feature, which `moq-scene` leaves off with its other
  defaults, builds CEF's wrapper with it.
- Prove it the way `a_renderer_cannot_read_the_user_s_files` in
  `rs/moq-studio/tests/browser.rs` does on Linux and macOS: a Windows
  `can_read` that asks whether the renderer's restricted, untrusted-integrity
  token may open a file the test wrote, where the helper's may.

## Hosts

- windows - CEF's Windows sandbox

## Required

- [Browser sources on Windows](/quest/m0/browser-source/windows.md) - browser sources run on Windows at all
