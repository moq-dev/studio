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
Renew over the transport's own re-authentication; do not invent a guest
protocol. A host's Remove from scenes should stop renewal, so the expiry
bounds a removed guest again. Raised on
[#10](https://github.com/moq-dev/studio/pull/10).

Found 2026-11-03: moq's `main` already renews in band, so nothing lands in
moq first. The join page adds the renewed token to its connection's auth
handle (`connection.auth.add`), which presents it on the live session and on
every reconnect, and closes the old credential once the new one's grant
arrives; the relay verifies it through its auth server
([in-band renewal](https://github.com/moq-dev/moq/blob/main/doc/bin/relay/auth.md#in-band-renewal)).
It needs moq-lite-07 or moq-transport draft-18+ with MoQ Auth on both the
page and the relay, which is what Required waits on.

Recommendations for the delivery track:

- moq-room's claims let every guest subscribe to `**`, so any guest could read
  another's renewal. Sign guests a narrower subscribe grant that covers their
  own token path, plus what the [return feed](/quest/m0/guest-return.md)
  publishes for them.
- The studio's own `--connect` session need not publish in the room. Publish
  the token broadcasts over a session dialed with a token the room key signs
  for those paths alone.
- Each renewal lasts one `--guest-expiry` and goes out well before the
  current `exp`, so a removed or departed guest stays bounded by one expiry.
  The studio holds at most one token broadcast per guest it signed for.

## Required

- [In-band token renewal is released](/quest/m0/auth-renewal-release.md) - the join page's `@moq/net` and the relay renew a token on a live session
