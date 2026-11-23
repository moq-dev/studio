# [M] Streamlined Go Live, auto-saving Settings, and clean Command Palette

## Goal

Cut the friction out of going live, ending a stream, editing settings, and
using the command palette ([mock-up](/quest/m0/ux/mock/overhaul-popovers.webp)):

- **One-click Go Live & inline first-time setup:**
  - Encoder and destination health run silently in the background; the health
    pill stays quiet when everything is OK and warns only when degraded.
  - Once at least one destination is configured, clicking `GO LIVE` starts
    streaming immediately without blocking on a multi-second benchmark modal
    every time.
  - When no destination is configured yet, clicking `GO LIVE` opens a single
    inline popover right under the button with Destination URL + optional
    key/token + `Go Live Now` (2 interactions instead of an 8-step detour
    through Settings > Stream > Save > Close > Go Live).
- **Discoverable End Stream:** clicking `LIVE <clock>` opens a compact
  confirmation popover (`End stream` / `Keep live`) instead of requiring a
  hidden 1.2-second press-and-hold.
- **Clear Recording control:** label the title bar's recording button clearly
  (`● REC` / `■ 00:12:04`) instead of a tiny unlabeled blue circle.
- **Consistent auto-save in Settings:** every Settings pane (`Show`,
  `Destinations`, `Recording`, `Hotkeys`, `Guests`, `Mods`) saves edits
  automatically as you change them, removing the manual `Save` / `Revert`
  buttons in `Stream` and `Guests`. Tuck `Failover relay` and
  `Encode only while someone watches` under a collapsed `Advanced` section in
  `Destinations`.
- **Clean Command Palette (`Ctrl+K`):** hide actions that cannot run right now
  (such as `End stream` off air or Studio Mode commands when Studio Mode is
  off) instead of leading the default list with amber `"Cannot now"` errors.

## Plan

Decided 2026-11-22 with the maintainer (/quest-plan):

- **Go Live flow:** ✅ silent background health checks + 1-click Go Live when
  configured + inline URL/token popover on first use (rejected: blocking
  encoder and 6-second uplink probes on every Go Live click; forcing an 8-step
  Settings detour when unconfigured). If a background check detects an encoder
  overload or unreachable relay, the popover opens with the one-line fix
  (for example `Send 720p30 instead`).
- **End Stream:** ✅ click `LIVE <clock>` -> explicit `End stream` confirm
  popover (rejected: hidden 1.2s press-and-hold with no click fallback).
- **Settings persistence:** ✅ immediate auto-save across all Settings panes
  with validation inline, matching Settings > Show (rejected: manual `Save` /
  `Revert` buttons in Stream and Guests where pressing `Esc` silently drops
  edits).
- **Command Palette:** ✅ show only actionable commands by default (matching
  enabled Mods and current on-air state) so the top of the palette is
  immediately useful (rejected: cluttering the top of the list with disabled
  amber warnings).
- **Shots & docs:** update `just shots` (`golive_inline`, `end_confirm`,
  `settings_destinations`, `palette`) and update `README.md`.

## Related

- [Unified Studio shell](/quest/m0/ux/unified-shell.md) - the slimmed title bar hosting REC, Health, and GO LIVE
- [Destinations](/quest/m0/destinations.md) - RTMP/WHIP destinations integrated into the same inline popover and auto-saving Destinations pane
