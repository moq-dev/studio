# [L] Toolkit spike for the redesign

## Goal

The LIVE page of the [mock-up](/quest/m0/ux/mock/shell-live.webp) runs twice,
once on egui and once on [Slint](https://slint.dev): the title bar with its
pages, health dot, and live control, the live picture in its tally frame, the
scene pads, and the sound strip, on the real compositor. Its PR reports what
each cost and recommends one, and the maintainer picks the toolkit every other
quest in this epic builds on.

## Plan

Decided 2026-11-09 (maintainer, /quest-plan):

- ✅ **Spike before any page is built** (rejected: staying on egui unasked;
  moving unasked).
- ✅ **egui and Slint** (rejected: egui and Rune, the first pick; all three;
  egui, Slint, and zgui). A survey of twelve Rust toolkits the same day put
  one requirement first: drawing the compositor's own wgpu textures with no
  copy, on the device the studio opens, which carries an external-semaphore
  extension only an app-created device can have
  (`moq/rs/moq-video/src/render/device.rs`). On wgpu 30, egui passes
  (`WgpuSetup::Existing`, `register_native_texture`) and so does Slint
  (`WGPUConfiguration::Manual`, an `Image` from a `wgpu::Texture`). Nothing
  else surveyed does.

The others, on paper only; the report says what changed for each since:

- **Rune** was spiked on 2026-11-07,
  [moq-dev/studio#98](https://github.com/moq-dev/studio/pull/98), which said
  stay on egui. The redesign has no dock, one of its costs there. The rest
  stand: it cannot take the studio's device without a patch, imports no
  texture without a copy on Linux, takes no pull requests, and has no
  release.
- **zgui** is built for this, with a device taken from the app and a surface
  element, but is on wgpu 29 and is a 0.1.0. Look again when it reaches
  wgpu 30.
- **GPUI, Xilem, Floem, Freya, Vizia, Makepad, Iced, and Blitz** fail the
  texture requirement or meet it only on an unreleased branch or their own
  device.

[A critique of GPUI](https://blog.zortax.de/posts/a-critique-of-gpui) prompted
the survey. Its case against immediate mode is cost that follows the size of
the UI rather than the change. That weighs little here: this window repaints
every frame for live video and meters, and its lists are tens of rows. What it
does raise is tested below.

Both halves stay off `main`. The PR deletes this quest and links both
branches.

The report compares, on the same scene and host:

- **Fidelity:** each half's screenshot beside the mock-up frame, and what
  could not be matched. Count what each toolkit lacks and the studio would
  own: the pads, the segmented meters at the display's rate, popovers and
  tooltips, a custom title bar with the window's drag region and controls on
  each desktop, text in the three faces. Slint has no paint callback in Rust,
  so say what a meter and a fader cost in its language.
- **Layout:** the centred pages in the title bar, a popover placed against
  its anchor, and a grid of cards, each right on its first frame. Immediate
  mode places a thing before it knows its size; say what each case took.
- **State:** how the engine's scene graph reaches the UI. egui reads it each
  frame; Slint needs it pushed into properties. Count the glue, and what a
  missed update looks like.
- **Frame time**, measured as AGENTS.md requires (a real compositor, not
  Xvfb), with accessibility on and off, and that the program's render time
  is unchanged.
- **Windows:** a second native window sharing the compositor's textures,
  which [multiview](/quest/m1/multiview.md), the
  [mini controller](/quest/m1/mini-controller.md), and
  [panels](/quest/m1/panels.md) need, and an always-on-top borderless one.
- **Testing:** what a headless run can assert and screenshot, which decides
  how [screenshots](/quest/m0/ux/shots.md) work.
- **Risk:** what each asks of upstream and what a year of maintenance looks
  like. For Slint: its wgpu feature is marked unstable and outside its
  version promise, and its license is GPLv3, royalty-free with attribution,
  or commercial, where the studio is MIT or Apache-2.0. Say plainly what each
  license would require of the studio and of anyone who redistributes it.

No fixed pass bar: the maintainer decides from the report. Hand the PR back
with that decision under `## Waiting`, and record the answer in
[the epic](/quest/m0/ux/README.md) and m0's README, whose 2026-10-22 entry
chose egui with egui_dock.

## Related

- [The look](/quest/m0/ux/kit.md) - the first quest the choice unblocks
