# [S] Add shows pictures

## Goal

Add's device, screen, and window cards show what each will capture, as the
[mock-up](/quest/m0/ux/mock/overhaul-add.webp) and
[`mock/add.html`](/quest/m0/ux/mock/add.html) draw: a live thumbnail for each
camera, display, and window, each window's app icon, and a list that updates
when a camera or monitor is plugged in or out or a window opens or closes,
with no `Refresh` button. Where the system cannot give a picture, the card
keeps its name and says why.

Audio inputs are out of scope: Add lists them each time it opens, and a
microphone plugged in while it is open shows the next time, until moq-audio
reports device changes.

## Plan

Decided 2026-11-27 (maintainer, /quest-plan): Add becomes a row of buttons
([Add is a row of buttons](/quest/m0/ux/add-buttons.md)), so these pictures
go in the studio's own Camera, Screen, and Window pickers rather than on cards
in a tab. On a Wayland desktop the system's picker shows screens and windows,
so only cameras get pictures there.

Split from Add's quest on 2026-11-25 (maintainer, /quest-run), so Add could
land on today's capture listing
([moq-dev/studio#262](https://github.com/moq-dev/studio/pull/262)) while
these wait on moq-video's richer one. Thumbnails come at the low rate that
listing offers, never from a full-size capture of every device; measure what
an open Add costs in frame time and CPU with a few cameras and windows listed.

## Required

- [moq-video's capture listing is pinned](/quest/m0/pin-capture-list.md) - icons, live pictures, and change notices for windows and devices
