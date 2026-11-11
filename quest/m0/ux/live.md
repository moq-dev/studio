# [XL] One live picture

## Goal

The studio opens to the LIVE page of the
[mock-up](/quest/m0/ux/mock/shell-live.webp), in its own frame:

- **The picture** is the program, in a red tally frame, as large as the
  window allows. The Preview monitor and the Program monitor are gone.
- **Scene pads** show each scene's live thumbnail, name, and key. A click
  takes the scene on air by the show's transition.
- **The sound strip** has a meter and a mute for each sound the stream
  carries, and the stream's own meter.
- **The title bar** is the window's: the show's name, undo and redo, the
  pages, one health dot, record, and the live control. The
  [hover frame](/quest/m0/ux/mock/shell-hover.webp) shows what each opens.
- **The panic key** cuts to a scene the show marks as its safe scene and
  mutes every microphone, from a button or a hotkey.
- **The layout follows the picture:** a
  [portrait show](/quest/m0/ux/mock/shell-portrait.webp) moves the pages to a
  sidebar and the scenes and sound to columns, and a
  [narrow window](/quest/m0/ux/mock/shell-compact.webp) stacks them. Nothing
  clips at any size the window allows.

The title bar and the bottom strip are on every page, so the scenes, the
sound, the panic key, and a live picture never leave the window while on air.

BUILD, MIX, and REVIEW exist as pages holding today's panels in the new look,
so nothing the studio does is lost before its own quest redesigns it.
egui_dock, or whatever the toolkit's dock is, is removed.

## Plan

Decided 2026-11-09 (maintainer, /quest-plan); see
[the epic](/quest/m0/ux/README.md) for the options rejected.

- ✅ **No Preview.** The compositor keeps its preview target
  (`Compositor::set_preview` in `rs/moq-scene/src/compositor.rs`) for
  [editing off air and Studio mode](/quest/m0/ux/studio-mode.md); on this
  page the edited scene is always the scene on air. Everything that targets
  `preview_scene` today (the Items panel, arrange, nudge, interact) targets
  the program's scene.
- ✅ **A click takes.** The refusals stay and are said in place: a scene of
  another size, or one that draws the program, cannot go on air
  (`Collection::takes`), so its pad says why instead of failing in a status
  line.
- ✅ **One live control.** Until [go live](/quest/m0/ux/go-live.md) lands,
  the studio is still live from launch when `--connect` is given, so the
  control shows the clock and its END STREAM face is not offered.
- ✅ **One health dot**, the worst grade of `health.rs`, with destinations,
  viewers, and the encoder in its popover.
- ✅ **Pages in the title bar for a landscape show, a left sidebar for a
  portrait one** (rejected: along the bottom).
- The hotkey actions (`rs/moq-studio/src/controls.rs`) keep working: a
  scene's key takes it, as its pad does. Cut and Transition have nothing to
  act on until Studio mode, so they are hidden until then.
- The panic key's safe scene and the mute are new state in the scene file;
  document both in `README.md`.

The layout is chosen by one pure function of the window's size and the
picture's shape, tested without a window: for every size from the minimum up,
no region overlaps another and the picture is the largest rectangle. The
mock-up's three arrangements are its cases.

Draw the pages from a snapshot of the engine, not while holding its lock
(`Studio::ui` holds `engine.state` across the whole dock today, and the
render thread wants the same lock). Split `ui.rs`, 4,900 lines behind one
`Panels` struct, by page as each is touched.

Measure the UI's frame time against today's under a real compositor, and
that the program's render time is unchanged.

Human check: taste and feel on the maintainer's machine, a second monitor and
mixed display scales, and the custom frame's drag, resize, snap, and
full-screen behavior on each desktop, which no headless run exercises.

## Required

- [The studio's look](/quest/m0/ux/kit.md) - the widgets the page is built from
