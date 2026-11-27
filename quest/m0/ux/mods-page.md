# [M] Mods is a page in the title bar

## Goal

Mods is a top-level MODS page beside STUDIO in the title bar, always there
and not something a user can turn off; Settings no longer holds it. The page
sells each mod: a card with its name, one line on who it is for, and a
picture of what it adds, laid out to look dramatically better than a list of
switches, since it is how streamers find what the studio can do. An enabled
mod's card has a **Show me** button that goes to where the mod appears and
highlights what it added for a moment.

With it:

- Guest rooms become a mod, off by default: the Guests pane and Add's Guest
  button appear only when it is on.
- The Scene graph mod adds a GRAPH page beside STUDIO, instead of its own
  window.
- Audio scenes & routes is no longer a mod: [Audio is layers](/quest/m0/ux/audio-layers.md)
  replaces it with a "Separate recording mix" mod.

## Plan

Found trying Add on 2026-11-27 (maintainer, [Try Add](/quest/m1/try-add.md)).

Decided 2026-11-27 (maintainer, /quest-plan):

- ✅ A MODS tab in the title bar, always shown (rejected: a "Show Mods" mod
  putting a button under Layers or beside the gear; staying in Settings).
  The maintainer: "This is the difference maker."
- ✅ **Show me** on an enabled mod's card navigates there and highlights it
  briefly (rejected: an accent dot on each new thing until first used; a
  where-to-look line alone).
- ✅ Guest rooms are a mod, off by default, switching itself on for a show
  that already has a room, as the other mods do (rejected: on by default).
  Most streamers stream solo.
- ✅ The scene graph is a page beside STUDIO.

Today: ten mods, all off by default (`rs/moq-studio/src/mods.rs`), drawn in
Settings (`rs/moq-studio/src/ui/mods.rs`,
[moq-dev/studio#235](https://github.com/moq-dev/studio/pull/235)); the scene
graph opens a window (`ui/mods.rs`). The command palette and a blocked
hotkey's message point at "Settings, Mods": point them at the page.

Recommendation for the pictures: one `just shots` view per mod showing what
it adds, saved as small checked-in images the page embeds, and a test that
every mod has one. Measure what they add to the binary's size and say it in
the PR.

Draw the page in [`mock/overhaul.html`](/quest/m0/ux/mock/overhaul.html)
(its `mods` frame shows today's Settings pane) before building it, and update
[the epic's decisions](/quest/m0/ux/README.md), which put Mods in Settings.

## Related

- [Try the Settings window](/quest/m1/try-settings.md) - the maintainer's try of Settings, which loses its Mods pane
