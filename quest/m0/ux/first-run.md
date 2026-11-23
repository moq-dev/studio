# [M] Zero-gate first run and auto-saved shows

## Goal

Launching `moq-studio` with no arguments opens straight into a working,
auto-saved Studio window ([mock-up](/quest/m0/ux/mock/overhaul-first.webp)),
with zero modal walls:

1. **First launch:** auto-creates a default show inside app data (no Library
   landing wall, no OS Save File dialog for `show.toml`, and no blocking
   5-canvas diagnostic benchmark screen). Encoder and canvas defaults
   (e.g. 1080p30/60 based on hardware encoder availability) are picked silently
   in the background.
2. **Empty-canvas quick-start:** when the active scene has no layers, the
   canvas itself shows four one-click starter cards (`Camera`, `Screen`,
   `Media`, `Web / Text`) plus a drag-and-drop/paste hint. One click adds the
   first source and immediately turns into the live editable scene.
3. **Subsequent launches:** reopens the last active show automatically.
4. **Show management:** switching shows, creating a new show, renaming, or
   exporting/importing a standalone `.toml` show file lives in the title bar's
   Show menu (and `moq-studio <path.toml>` still opens an explicit file path).

It replaces the empty Library wall (`rs/moq-studio/src/library.rs`), the
upfront OS Save File dialog, and the raw diagnostic setup table
(`rs/moq-studio/src/setup.rs`).

## Plan

Updated 2026-11-22 with the maintainer (/quest-plan):

- **Startup flow:** ✅ zero-gate startup straight into an auto-saved show in
  app data with empty-canvas quick-start cards (rejected: 1-screen visual
  wizard before Studio; keeping the Library landing screen and 4-step wizard;
  forcing an OS Save File dialog before the user sees the app).
- **Background probe:** ✅ pick a sensible default canvas (1080p at 30 fps,
  upgrading to 60 fps when a hardware encoder is present) without blocking
  startup on five sequential benchmark encodes; if the encoder cannot keep up
  when going live, surface a single non-blocking health suggestion (rejected:
  blocking startup on a 5-canvas diagnostic table).
- **Default audio:** ✅ if a default system microphone exists, wire it cleanly
  into the bottom audio dock automatically (or show a 1-click `+ Mic` pill) so
  first-time users see live levels right away.
- **Show menu:** ✅ `Show v` in the title bar lists recent/saved shows,
  `New show` (auto-named in app data, no Save dialog), `Open show file...`,
  and `Export show copy...` (for users who want to share or version-control a
  `.toml` file).
- **Shots & docs:** replace the `setup` / `library` shots with `first_launch`
  (empty canvas quick-start) and `show_menu` (switching/creating shows), and
  update `README.md`.

Human check: launch fresh with an empty app-data directory on a machine with a
real camera and mic, and confirm going from cold launch to a live camera +
screen takes two clicks total.

## Related

- [Unified Studio shell](/quest/m0/ux/unified-shell.md) - the workspace this opens directly into
- [Adding a source in one click](/quest/m0/ux/add.md) - the picker opened by the quick-start cards
