# [M] Browser sources on Windows

## Goal

Browser sources run on Windows as the
[epic](/quest/m0/browser-source/README.md) describes, each page's shared
texture imported into wgpu as a D3D11 shared handle.

## Plan

On top of what Linux landed:

- A channel without Unix sockets, such as the helper's stdio pipes. The
  studio duplicates CEF's NT handle out of the helper with `DuplicateHandle`
  before `cef-rs` imports it.
- Windows has no process groups: start the helper in a job object with
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, which the studio's handle keeps
  alive.

## Required

- [Studio on Windows](/quest/m0/app-windows.md) - the studio runs there at all
