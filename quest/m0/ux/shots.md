# [M] UI screenshots on demand

## Goal

One command renders named views of the studio to PNG files with no display:
each page, and each state a quest names, on a scene file kept in the
repository. A view is picked by name, never by clicking at coordinates, and
two runs of a still view give the same picture. Every UI pull request shows
its screenshots beside the mock-up frames it implements, and CI keeps the
command working.

## Plan

The UI has no screenshot or snapshot test today. /quest-plan built a throwaway
harness on 2026-11-09 that proves the shape and shows what is missing:

- It ran the studio in a private headless sway and captured with grim, driving
  a hand-written virtual pointer over the Wayland wire protocol, because the
  layout is fixed in `Studio::new` and a panel can only be reached by clicking
  its tab.
- The dev shell has no sway, grim, or input tool. Outside NixOS, Nix-built
  sway cannot find the host's GL and Vulkan drivers without four environment
  variables.
- Meters, clocks, and playing clips never settle, so a capture catches an
  arbitrary moment.
- One launch in ten crashed; see
  [The studio starts every time](/quest/m0/ux/surface-crash.md).

So the app takes the view to show as an argument or an environment variable,
and runs on a paused `kio::clock` with still sources for a screenshot. The
[epic](/quest/m0/ux/README.md) chose egui, so the picture comes from
[`egui_kittest`](https://crates.io/crates/egui_kittest), which renders
headless on wgpu, finds widgets through the AccessKit tree, and compares
snapshots. It needs egui 0.37, which the studio reaches once `egui_dock`
(pinned to 0.36) is gone; until then the spike's headless sway with grim
still works. Either way the views are
named in one table that quests add to.

AGENTS.md's rule stands: frame time is measured under a real compositor.
Screenshots are for looking, not timing.

Human check: none. The maintainer reads the pictures in later PRs.

## Required

- [The studio starts every time](/quest/m0/ux/surface-crash.md) - a launch that can crash cannot run in CI
