# [M] Essential layer inspector and direct canvas editing

## Goal

Streamline layer editing so selecting a layer feels immediate and uncluttered
([mock-up](/quest/m0/ux/mock/overhaul-edit.webp)):

- **Contextual inspector:** the right-hand Inspector opens only while a layer
  is selected (closing cleanly when clicking empty canvas or pressing `Esc`),
  giving the canvas full width by default.
- **Zero duplicated controls:** keep direct canvas actions (`Crop`, `Fit`,
  `Fill`, `Lock`, `Delete`) on the floating canvas bar, and remove those
  duplicate buttons from the Inspector.
- **Essential controls first:** the default layer Inspector shows only:
  1. `Source` (live dropdown to swap camera, screen, or file in one click)
  2. `Shape` (rectangle, rounded, circle, arch + `Border` and `Shadow`)
  3. `Remove background` toggle and `Opacity` slider
  4. Collapsed `▸ More` disclosure for advanced geometry (`X`/`Y`/`W`/`H`),
     labeled `Crop` (`Top`/`Right`/`Bottom`/`Left`), `Blend` mode, `Filters`,
     and `Color grade` (hiding `Color grade` entirely when no LUTs exist).
- **Keyboard deletion:** bind `Delete` and `Backspace` to remove the selected
  layer (undoable with `Ctrl+Z`), instead of requiring a 14-item right-click
  context menu.
- **Trim context menus and move global Transition:** shrink the layer context
  menu to core actions (`Hide`, `Lock`, `Bring forward` / `Send backward`,
  `Duplicate`, `Remove`), hiding `Slots`, `Keep out of the way`,
  `Linked / Make independent`, and `Published independently` behind their Mods.
  Move the show-wide `Transition` picker out of the unselected scene inspector
  into Settings > Show (and the scene strip's menu).

## Plan

Decided 2026-11-22 with the maintainer (/quest-plan):

- **Inspector visibility & depth:** ✅ contextual Inspector on selection with
  essentials on top and `▸ More` collapsed by default, moving `Slots`,
  `Keep out of the way`, `Linked/Independent`, and `Published independently`
  to Settings > Mods (rejected: keeping all 5 sections always expanded;
  removing the right-hand inspector completely in favor of popovers only).
- **Toolbar deduplication:** ✅ floating canvas toolbar owns `Crop`, `Fit`,
  `Fill`, `Lock`, and `Delete`; Inspector owns `Source`, `Shape`,
  `Remove background`, `Opacity`, and `▸ More` (rejected: duplicating `Crop`,
  `Fit`, `Fill`, `Shape`, `Cut out`, and `Lock` in both places).
- **Crop labels & LUT hygiene:** ✅ label the four crop fields `T`/`R`/`B`/`L`
  inside `▸ More` and hide `Color grade` when no `.cube` LUTs are present.
- **Shortcuts:** ✅ `Delete` / `Backspace` deletes the selected layer when no
  text field has focus; `Esc` exits crop mode first, then clears selection.
- **Shots & docs:** update `just shots` (`edit`, `edit_more`) and update
  `README.md`.

## Related

- [Adding a source in one click](/quest/m0/ux/add.md) - adds and selects new layers directly into this flow
