# [L] Mods: features most streamers never use, off until enabled

## Goal

A new Settings pane, **Mods**, lists features most streamers never use, each
off by default: Both formats, Studio mode, Multiview, Phone remote, Published
scenes, and Scene graph. While a mod is off, no page, toolbar, menu, hotkey
list, palette, or Settings pane shows it. Switching it on shows its UI at
once, with no restart. All six are compiled into the binary. A mod is not a
plugin: there is no plugin API, no cargo feature flags, and nothing to
install.

Settings > Advanced is gone: Published scenes and Scene graph move to Mods,
and the Virtual camera toggle moves to Settings > Stream.

## Plan

Decided 2026-11-20 with the maintainer (/quest-plan). The goal is simplicity:
the window a new streamer sees holds only what most streamers use.

- **Pane name:** ✅ Mods (rejected: Extras, Features). The maintainer's word,
  even though these are built-in features rather than add-ons.
- **Which:** ✅ Both formats (portrait and dual format: the Format cards and
  Both at once in Settings > Show, the format chips, and portrait
  arrangements), Studio mode (the cue/take switch, NEXT, TAKE, AUTO, and their
  hotkeys and palette entries), Multiview (the show-menu and palette entries
  and the window), Phone remote (the Remote pane and the phone page), Published
  scenes (scene sizes, publishing a scene or source as its own broadcast, the
  inspector's Publish, and the per-scene virtual camera toggles), and Scene
  graph (the window and its palette entry) (rejected: Keyframes and Keep out of
  the way, which show only for a selected layer and cost nothing until used).
- **Storage:** ✅ per user, in the app storage beside hotkeys and Studio mode
  (rejected: per show file, since Studio mode, Multiview, and Phone remote are
  about how the operator works, not about the show; refusing a show that uses
  a disabled mod). Opening a show that uses a disabled mod, such as one whose
  scene file has portrait on or a published scene, switches that mod on, so a
  mod's UI is never hidden while its feature affects the output.
- **Switching off:** ✅ switching off a mod turns its feature off (rejected:
  refusing until the feature is off; only hiding the UI). Show edits, such as
  Both formats dropping portrait-only layers or unpublishing a scene, go
  through undo like the feature's own switch. Phone pairings are kept but stop
  working until the mod is back on. On air, it asks what the feature's own
  switch already asks, such as "Restart the video?", and adds no dialog of its
  own (rejected: confirming every switch-off; no prompt even on air). An open
  Multiview or graph window just closes.
- **Advanced:** ✅ the Virtual camera toggle moves to Settings > Stream and the
  Advanced pane is deleted (rejected: Virtual camera as a seventh mod; keeping
  Advanced for one row).
- **Shape:** one definition per mod (its name and help line), consulted where
  each entry point is drawn. Keep it as small as a set of flags in app storage;
  no registry or trait unless the code asks for one.

Also in this PR:

- `quest/m0/ux/mock/settings.html` gets a Mods pane in place of Advanced, and
  its frames are re-rendered, as the [UX epic](/quest/m0/ux/README.md)
  requires of a child that departs from a mock-up.
- `README.md` gains a Mods section and tells each mod's section to enable it
  first; Settings > Advanced references move.
- `just shots` gains a view of the Mods pane, and the views of each mod's UI
  enable that mod.
- The try quests that exercise a mod say to enable it first:
  [dual format](/quest/m1/dual-format-check.md),
  [multiview](/quest/m1/try-multiview.md),
  [phone remote](/quest/m2/try-phone-remote.md), and the pane list in
  [Try the Settings window](/quest/m0/ux/try-settings.md).

## Related

- [Broadcaster UX](/quest/m0/ux/README.md) - the "In Settings" column this changes
