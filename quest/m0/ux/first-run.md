# [M] First run

## Goal

A new show is made in four pictures, as in
[`mock/firstrun.html`](/quest/m0/ux/mock/firstrun.html):

1. **What are you making?** Landscape or portrait.
2. **What goes in it?** A camera, a screen or window, and a microphone, each
   picked from live pictures and a meter that says "say something", or none.
3. **Can this machine and connection carry it?** One verdict in large type,
   such as "Ready for 1080p at 60", with the three checks as short lines and
   the measurements behind Details. A check that failed is one plain sentence
   and one button that fixes or skips it.
4. **A starting layout**, from a few pictured templates, then Start.

It replaces guided setup's screen of findings, which shows a failed check as
a red line of the error's own text and needs `--connect` on the command line
to measure the uplink.

## Plan

The probes and the canvas choice are guided setup's
(`rs/moq-studio/src/setup.rs`); this is how they are shown and reached.

- It runs from New show in the library, not only when a named file is
  missing.
- The connection check uses the relay set in the same flow: a field for its
  address with the default filled in, skippable, in which case the verdict
  says the canvas was chosen without measuring.
- The templates are scene files in the repository with places for the
  picked camera, picture, and microphone, and each works with any of them
  missing.
- Where the operating system owns the picker, step 2's screen or window is
  the Add page's one button that opens it, not a row of thumbnails.
- The `check` frame draws a failed check beside a passing one to show how it
  reads; the screen shows one verdict.
- Nothing is written until Start, as today, and the proposed scene is the
  first edit in the history.

Human check: a first run by someone who has not used the studio, on a
machine with a real camera and microphone, watched without helping.

## Required

- [Adding a source by picking a picture](/quest/m0/ux/add.md) - the cards with live pictures and meters
- [Going live from inside the app](/quest/m0/ux/go-live.md) - the show library it starts from and the relay it measures
