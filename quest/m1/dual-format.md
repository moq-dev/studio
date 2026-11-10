# [L] Landscape and portrait at once

## Goal

A show can go out in both formats together. Each scene has a landscape and a
portrait arrangement of the same layers, the LIVE page shows the two
pictures side by side, each in its tally frame, and a scene pad takes both.
A layer says where it is shown: both, landscape, or portrait. The go-live
sheet sends each destination one format. See the `dual` frame of
[`mock/later.html`](/quest/m0/ux/mock/later.html).

## Plan

Decided 2026-11-09 (maintainer, /quest-plan): ✅ one format per show in m0
and both at once here (rejected: designing every m0 page for two pictures;
never planning it).

The engine can already do it the long way: a scene has its own size, can be
published as a broadcast of its own, and can draw the program through a
program source, cropped. This quest makes that one switch.

- Settle the model here: one scene with an arrangement per format, or two
  scenes linked by name. The first keeps a rename, a reorder, and a take from
  ever going out of step.
- A portrait arrangement starts from the landscape one: the main picture
  cropped around its subject, the camera stacked under it. It is then the
  streamer's to move.
- Each format is its own encode, so it costs an encoder session and its
  bitrate; the checklist says whether the machine and the uplink carry both.
- The pages follow: BUILD edits one arrangement at a time with the other
  beside it, and Studio mode cues both.

Human check: a phone and a desktop viewer watching the same show.

## Required

- [The BUILD page](/quest/m0/ux/build.md) - editing an arrangement per format
- [Going live from inside the app](/quest/m0/ux/go-live.md) - a format per destination
