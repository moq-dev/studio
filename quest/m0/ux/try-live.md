# [XS] Try the LIVE page

## Goal

The maintainer tries the LIVE page on their own machines and either signs it
off or files what they find as quests: what the epic's
[What a person must check](/quest/m0/ux/README.md) asks of it, which no
headless run exercises.

## Plan

The LIVE page, its strip, the panic key, and the title bar the studio draws
itself are on `main` (`cargo run -p moq-studio --features capture`, or the
macOS app's disk image, as [Try the macOS app](/quest/m0/packaging/macos-try.md)
says where to find it). What to try:

- **Look and feel:** whether the page reads as bold and calm at once beside
  the [mock-up](/quest/m0/ux/mock/shell-live.webp); taking a scene by its pad
  and by its key; the panic key's cut and mute, and undoing the mute.
- **Displays:** a second monitor, and moving the window between displays of
  mixed scales.
- **The custom frame** on each desktop it runs on (GNOME, KDE, macOS, and
  Windows once [Studio on Windows](/quest/m0/app-windows.md) runs it):
  dragging by the title bar, double-click to maximize, resizing from each
  edge, snapping to a screen edge, and full screen.
- **macOS:** the system title bar stays after first-run setup until the next
  launch, then the studio's own frame replaces it.
- **Going live** ([moq-dev/studio#175](https://github.com/moq-dev/studio/pull/175)):
  opening a show from the library, a real stream to a relay with your token
  set in Settings, Destinations (kept in the OS credential store; only Linux's
  Secret Service has been exercised, so macOS's keychain matters most), the
  first GO LIVE with no relay, asking for one in its popover, then one-click
  GO LIVE once the background check passes, ending from the clock's End
  stream popover, and the Go live and End stream hotkeys while another app is
  full screen.

An agent then turns each finding into a quest, or an edit to the quest that
owns that page, and deletes this one.

## Waiting

- Maintainer: try the LIVE page as the Plan lists and report what to change,
  or that nothing needs to. An agent cannot judge taste or drive a real
  desktop's window manager (the agent hosts run headless sway, which neither
  snaps nor draws a second display of another scale); a kittest run already
  covers the clicks. Check: a comment or quest records the result.
