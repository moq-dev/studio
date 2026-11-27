# [S] Layers and the inspector stay put

## Goal

Editing layers never jumps the picture or garbles the list:

- The right-hand inspector is always shown. With nothing selected it says
  no layer is selected, so selecting, deleting, or clicking away never
  resizes the picture.
- The Layers column's rows have breathing room: padded, rounded rows whose
  name and subtitle sit inside them at any font size, as
  [`mock/overhaul.html`](/quest/m0/ux/mock/overhaul.html) draws them.
- Every layer can be hidden with its eye, a screen capture included, and
  hiding takes effect at once rather than shrinking the layer to nothing.

## Plan

Found trying Add on 2026-11-27 (maintainer, [Try Add](/quest/m1/try-add.md)).

Decided 2026-11-27 (maintainer, /quest-plan): ✅ the inspector is always
shown, with an empty state when nothing is selected (rejected: keeping it
contextual, which [moq-dev/studio#256](https://github.com/moq-dev/studio/pull/256)
built; overlaying it on the picture). The picture moving on every select or
delete is worse than a slightly narrower picture. This reverses #256's
"opens only while a layer is selected", so update that line of
[the epic's decisions](/quest/m0/ux/README.md).

- Inspector: `inspects()` in `rs/moq-studio/src/ui/build.rs` shows the column
  only while a layer is selected or a mod needs it. An empty state already
  exists (`scene_inspector`, `rs/moq-studio/src/ui/inspector.rs`) behind
  Smart layout and Published scenes; show one with no mod on.
- Rows: `rs/moq-studio/src/ui/layers.rs` draws fixed 46 px rows with no
  spacing and paints text at fixed offsets
  ([moq-dev/studio#174](https://github.com/moq-dev/studio/pull/174),
  [moq-dev/studio#253](https://github.com/moq-dev/studio/pull/253)).
- Hide: the maintainer saw a picked display that would not hide, and a hidden
  image animate down to zero. The eye only sets `visible`, and no hide
  animation is in the studio's code, so reproduce both first, on a real
  desktop for the picked display (the Smart layout slot reflow in
  `rs/moq-scene/src/compositor.rs` is the one box animation to rule out).

Update `just shots` views for the inspector's empty state and the rows.

## Related

- [Try the Studio workspace](/quest/m1/try-studio.md) - the maintainer's try of the workspace these change
