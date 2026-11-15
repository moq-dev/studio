# [XS] Picked displays and windows are tried on a Wayland desktop

## Goal

The maintainer confirms on a real Wayland desktop that a `picked_display` and a
`picked_window` source show the desktop's screen-sharing picker once, show the
chosen screen or window, and reopen without the picker after the studio
restarts. If they don't, the quest is rewritten to say what failed.

## Plan

Build with `--features capture,pipewire`. On GNOME or KDE, both of which keep
grants, add **Picked display** from the Devices panel, choose a screen, quit,
and start again. The picker must not show the second time, and
`~/.local/state/moq-studio/portal.toml` holds one grant under the scene file.
Then stop sharing from the desktop's own indicator: the source fails, the
grant is gone from the file, and restarting the source shows the picker again.

An agent exercised the picker through xdg-desktop-portal-wlr with a scripted
chooser (moq-dev/studio#172), but no compositor it can run headless returns
restore tokens: wlr 0.8.5 has none, and Hyprland 0.57 cannot nest under
headless sway or weston. So the restore across restarts needs a person on a
desktop that keeps grants.

## Hosts

- linux - needs a Wayland desktop with xdg-desktop-portal

## Waiting

- Maintainer: run the check in Plan on a GNOME or KDE session, since only a
  person can click the portal's picker and no headless backend restores
  grants (automation tried: wlr, Hyprland). Done when the second start shows
  no picker.
