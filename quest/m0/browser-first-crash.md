# [S] A browser renderer that dies on its first page

## Goal

A browser source whose renderer dies during its first navigation shows as
failed, as one that crashes later does, instead of staying Starting forever.
`a_crash_drops_only_its_source` passes on NixOS as well as Ubuntu.

## Plan

From [moq-dev/studio#8](https://github.com/moq-dev/studio/pull/8) and
[moq-dev/studio#29](https://github.com/moq-dev/studio/pull/29).

- A renderer crashed by `chrome://crash` as the first URL fires neither
  `on_render_process_terminated` (`rs/moq-scene/src/browser/host.rs`) nor
  `on_load_error`, so the page stays Starting. Checked with debug prints on
  #29.
- Find the signal CEF does give for it (for example the browser's process
  exit or a load that never commits) and fail the source on it, without a
  timeout: fail loud on an observed event.
- Split the test: one crash during the first navigation, and one after the
  page commits.
