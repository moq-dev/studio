# [M] A capture that went black says so

## Goal

When a camera, window, or display source goes black or stops sending
pictures, its layer carries an amber badge on the picture and in Layers, and
a popover says what happened in plain words and offers the fix: pick the
window again, capture the whole screen instead, or reopen the device. The
health dot counts it. See the `capturehealth` frame of
[`mock/later.html`](/quest/m0/ux/mock/later.html).

## Plan

A black capture is the commonest complaint about OBS, where the picture is
simply black and the cause is a forum search away.

- A stalled camera or broadcast is already detected (no new picture for a
  second). A window or display that stands still is not a fault, so those
  are judged by content: a picture that is all one dark color for a
  sustained time while its source claims to be live.
- The check reads a far smaller copy of the frame at a low rate, on the GPU.
  Measure its cost per source.
- Each fix is an action the studio can take itself; where it cannot (a
  permission was revoked, a game refuses capture), the popover says what the
  streamer must do, and where.
- Recommended, not decided, what viewers see meanwhile: the last good
  picture for a few seconds, then the black capture as it is, so a hiccup is
  invisible but a stale frame never passes for live. The alternatives are
  black at once, as today, or a placeholder the show chooses.
- A source that is meant to be black can dismiss the badge for good.

## Required

- [The BUILD page](/quest/m0/ux/build.md) - the layer rows and the on-picture badge
