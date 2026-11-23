# [L] Mods: features most streamers never use, off until enabled

## Goal

A new Settings pane, **Mods** ([mock-up](/quest/m0/ux/mock/overhaul-mods.webp)),
lists features most streamers never use, each off by default:

- **Audio Scenes & Routes:** multi-bus audio routing (the full `MIX` desk,
  `S`/`H`/`R` send dots, and the `Routes` matrix). When off, every unmuted
  sound feeds the stream and recording directly, controlled by inline sliders
  and a per-source headphones monitor toggle.
- **Replay & Review:** the `REVIEW` page, instant replay buffer, clip trimming,
  and live stream markers (plus clip/marker buttons in the dock).
- **Smart Layout:** scene `Slots` (Main/Camera tile arrangements) and layer
  `Keep out of the way` automatic dodging.
- **Both formats:** portrait and dual-format broadcasts (Format cards, Both at
  once, format chips, portrait arrangements).
- **Studio mode:** cue and take preview (`NEXT`, `TAKE`, `AUTO`, and their
  hotkeys and palette entries).
- **Multiview:** the multi-scene/source monitor window.
- **Phone remote:** the Remote pane and phone controller page.
- **Published scenes:** per-scene sizes, independent broadcast publishing,
  viewer-toggleable layers, priority, and per-scene virtual camera toggles.
- **Scene graph:** the scene graph inspection window.

While a mod is off, no page, toolbar, menu, hotkey list, palette, or Settings
pane shows it. Switching it on shows its UI at once, with no restart. All nine
are compiled into the binary. A mod is not a plugin: there is no plugin API,
no cargo feature flags, and nothing to install.

Settings > Advanced is gone: Published scenes and Scene graph move to Mods,
and the Virtual camera toggle moves to Settings > Stream.

## Plan

Decided 2026-11-20 and expanded 2026-11-22 with the maintainer (/quest-plan).
The goal is simplicity: the window a new streamer sees holds only what most
streamers use.

- **Pane name:** ✅ Mods (rejected: Extras, Features). The maintainer's word,
  even though these are built-in features rather than add-ons.
- **Which (updated 2026-11-22):** ✅ Audio Scenes & Routes, Replay & Review,
  Smart Layout (Slots + Keep out of the way), Both formats, Studio mode,
  Multiview, Phone remote, Published scenes, and Scene graph (rejected:
  leaving them on the default pages and in Advanced; keeping Audio Scenes on
  by default in Mix; keeping Review as a permanent top-level tab off air;
  keeping Slots and Keep out of the way in every user's inspector).
- **Storage:** ✅ per user, in the app storage beside hotkeys and Studio mode
  (rejected: per show file, since Studio mode, Multiview, and Phone remote are
  about how the operator works, not about the show; refusing a show that uses
  a disabled mod). Opening a show that uses a disabled mod, such as one whose
  scene file has custom audio scenes, portrait on, slots, or a published scene,
  switches that mod on, so a mod's UI is never hidden while its feature affects
  the output.
- **Switching off:** ✅ switching off a mod turns its feature off (rejected:
  refusing until the feature is off; only hiding the UI). Show edits, such as
  Both formats dropping portrait-only layers or unpublishing a scene, go
  through undo like the feature's own switch. Phone pairings are kept but stop
  working until the mod is back on. On air, it asks what the feature's own
  switch already asks, such as "Restart the video?", and adds no dialog of its
  own (rejected: confirming every switch-off; no prompt even on air). An open
  Multiview or graph window just closes.
- **Advanced:** ✅ the Virtual camera toggle moves to Settings > Stream and the
  Advanced pane is deleted (rejected: Virtual camera as a tenth mod; keeping
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
- [Unified Studio shell](/quest/m0/ux/unified-shell.md) - hides MIX and REVIEW tabs when their mods are off
