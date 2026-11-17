# [XS] Watch the slots reflow with real guests

## Goal

The maintainer has seen a scene laid out by slots reflow on a real show, with
guests joining and leaving mid-stream, and it reads as a deliberate glide
rather than a glitch. If it does not, the reflow's length, easing, or what
closes a slot changes until it does.

## Plan

The geometry, the glide's frames on the program clock, and a guest taking a
tile are tested in moq-scene and the studio, and BUILD's views show the
slots. Whether the motion reads well is a judgment no test makes, so it waits
on a person watching.

## Waiting

- Maintainer: run a show with a scene laid out by slots on program and
  **Appear on their own** on (Settings, Guests), have two guests join from
  the join page and one leave, and watch the program and a recording. An
  agent cannot judge how motion reads to a viewer; the GPU tests already
  check the frames. Done when the glide reads as intended, or the change it
  needs is written into this Plan.
