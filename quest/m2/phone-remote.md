# [L] A phone as a remote

## Goal

A QR code in the studio opens a page on a phone that shows the scene pads,
the mutes, the live clock, and a tally, and drives them. No app is
installed, and it works wherever the phone reaches the relay, as a guest's
join page does.

## Plan

Planned 2026-11-09 (maintainer, /quest-plan), and held for later: ✅ m2
(rejected: m1).

- It is a page beside the join page (`js/join`), over the room the studio
  already owns: the studio publishes its state there, and the remote
  publishes commands the studio subscribes to.
- A remote's token is signed like a guest's and lets it do only that. Its
  commands are untrusted input from the network: a fixed, small vocabulary,
  rejected whole when malformed, and bounded in rate by design.
- Removing a remote stops renewing its token, as for a guest.
- The page follows the look of the [LIVE page](/quest/m0/ux/live.md) at a
  phone's width.

## Required

- [Going live from inside the app](/quest/m0/ux/go-live.md) - the relay and room it is reached through
