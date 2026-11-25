# [XS] Try the Studio workspace

## Goal

The maintainer tries the unified Studio workspace on their own machines and
either signs it off or files what they find as quests: what the epic's
[What a person must check](/quest/m0/ux/README.md) asks of it, which no
headless run exercises.

## Plan

The Studio workspace (the editable picture, the Layers rail and Editable
switch, the slim strip with each sound's slider, meter, and options, and the
title bar the studio draws itself) is on `main`
(`cargo run -p moq-studio --features capture`, or the macOS app's disk
image, as [Try the macOS app](/quest/m0/packaging/macos-try.md) says where to
find it). It replaced the LIVE and BUILD pages. What to try, beside the
[mock-up](/quest/m0/ux/mock/overhaul-studio.webp) and the
[audio popover](/quest/m0/ux/mock/overhaul-popovers.webp):

- **Look and feel:** whether the window reads as bold and calm at once;
  taking a scene by its pad and by its key; **+** switching to the new scene
  (taken off air, opened off air on air); the panic key showing only with a
  safe scene or on air, its cut and mute, and undoing the mute.
- **Editing:** dragging, resizing, and cropping a layer on the picture with a
  real mouse and a trackpad; restacking layers by dragging their rows; how
  snapping and the grid hold a drag; Escape ending a crop, then the
  selection; **Editable** locking the picture mid-show, and **Layers**
  hiding the rail.
- **Sound:** riding a sound's slider in the strip with a mouse and a
  trackpad, whether its taper is fine enough near 0 dB and a double click
  returns there; the meter under it moving with a real microphone; its gear's
  **Monitor in headphones** on real headphones, and its delay and pan.
- **Real sources:** a camera, a capture card, and a display already in the
  show, so the row's status dot and the inspector's Source section show a
  real device starting, failing, and coming back. Adding them from Add is
  left to Add's own try, since [Add](/quest/m0/ux/add.md) is being
  redesigned (decided 2026-11-25, maintainer, /quest-run).
- **Displays and the frame:** a second monitor, moving the window between
  displays of mixed scales, and the custom frame on each desktop it runs on
  (GNOME, KDE, macOS, and Windows once
  [Studio on Windows](/quest/m0/app-windows.md) runs it): dragging by the
  title bar, double-click to maximize, resizing from each edge, snapping to a
  screen edge, and full screen.
- **Going live:** a real stream to a relay with your token set in Settings,
  Destinations (kept in the OS credential store; only Linux's Secret Service
  has been exercised, so macOS's keychain matters most), the first GO LIVE
  with no relay, asking for one in its popover, then one-click GO LIVE once
  the background check passes, ending from the clock's End stream popover,
  and the Go live and End stream hotkeys while another app is full screen.
- **Interact:** typing into and clicking a web page layer under another page.

An agent then turns each finding into a quest, or an edit to the quest that
owns that part, and deletes this one.

## Waiting

- Maintainer: try the Studio workspace as the Plan lists and report what to
  change, or that nothing needs to. An agent cannot judge feel or taste,
  drive a real desktop's window manager, or hear headphones, and the agent
  hosts have no camera, capture card, or microphone; a kittest run already
  covers the clicks, and `just shots` the layout. Check: a comment or quest
  records the result.
