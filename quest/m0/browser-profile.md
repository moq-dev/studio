# [S] Browser sources keep a profile

## Goal

Browser sources share one persistent browser profile, so cookies and logins
(an alert service, a chat widget) survive a restart of the source or the
studio, as in OBS.
Each browser source also has a reload action that restarts its page without
editing the source.

## Plan

Decided 2026-11-02 (maintainer): one shared profile for all browser sources,
not one per source. Each page runs in its own CEF helper process today with a
fresh temporary profile, and two CEF instances cannot share a profile
directory, so sharing it means the helpers share one CEF instance or hand the
profile between them; weigh that against the crash isolation per page that the
helpers give. Keep the profile beside the studio's other per-user state, not
beside the scene file, since the scene file is meant to be shared. Raised on
[#8](https://github.com/moq-dev/studio/pull/8).

## Related

- [Browser sources](/quest/m0/browser-source.md) - the macOS and Windows half
