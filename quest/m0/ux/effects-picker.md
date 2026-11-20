# [S] Delete the Effects picker

## Goal

The **Effects** setting (Full, Reduced, Off) in Settings > Advanced is gone.
The window's effects always follow the system's reduced-motion setting, as
they already do until a user picks one. The effects themselves stay.

## Plan

Decided 2026-11-20 with the maintainer (/quest-plan): ✅ delete it (rejected:
moving it to Mods). It's a setting nobody looks for, and the system setting
already covers reduced motion on macOS, Windows, and Linux.

Remove the stored `"effects"` choice from app storage, the picker and its row,
and the README's sentence about it.
