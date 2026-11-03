# [M] Guest token refresh

## Goal

A show can outlast `--guest-expiry`: a joined guest's credential is renewed
while they stay in the room, without the host sending a new link and without
the guest reconnecting.

## Plan

A signed guest token cannot be revoked, and the relay ends a session at its
`exp`, which is why links expire today. The studio holds the room key, so it
signs each renewed token, and the token has to reach the connected guest
before `exp` over something the room already provides, such as a track only
that guest may subscribe to; the join page then re-authenticates with it.
Renew over the transport's own re-authentication if moq supports it, or land
that in moq first; do not invent a guest protocol. A host's Remove from scenes
should stop renewal, so the expiry bounds a removed guest again. Raised on
[#10](https://github.com/moq-dev/studio/pull/10).
