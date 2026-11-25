# [S] Add shows pictures

## Goal

Add's device, screen, and window cards show what each will capture, as the
[mock-up](/quest/m0/ux/mock/overhaul-add.webp) and
[`mock/add.html`](/quest/m0/ux/mock/add.html) draw: a live thumbnail for each
camera, display, and window, each window's app icon, and a list that updates
when a camera or monitor is plugged in or out or a window opens or closes,
with no `Refresh` button. Where the system cannot give a picture, the card
keeps its name and says why.

## Plan

Split from [Adding a source in one click](/quest/m0/ux/add.md) on 2026-11-25
(maintainer, /quest-run), so Add could land on today's capture listing while
these wait on moq-video's richer one. Thumbnails come at the low rate that
listing offers, never from a full-size capture of every device; measure what
an open Add costs in frame time and CPU with a few cameras and windows listed.

## Required

- [Adding a source in one click](/quest/m0/ux/add.md) - the modal and its cards
- [moq-video's capture listing is pinned](/quest/m0/pin-capture-list.md) - icons, live pictures, and change notices for windows and devices
