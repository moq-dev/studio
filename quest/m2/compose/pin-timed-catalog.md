# [S] hang's timed catalog is pinned

## Goal

The `moq` submodule includes hang applying each catalog update at its
timestamp, from moq's
[hang applies catalog updates at their timestamp](https://github.com/moq-dev/moq/blob/main/quest/m2/catalog-at-timestamp.md),
so `rs/hang`'s catalog consumer hands the studio each update with its
timestamp, which [the composition](/quest/m2/compose/layout.md) needs to move
into its broadcast's catalog.

## Plan

Once the wait clears, advance it with `just upgrade-moq` in its own PR,
fixing whatever the new API breaks here. That quest builds on moq's lite-07
time addressing (groups addressed by their start), so the pin may cross
that model change too, which is why it is sized above the usual pin.

## Waiting

- moq-dev/moq: the catalog-at-timestamp quest lands. Check: its PR has
  merged on moq's `main`, deleting `quest/m2/catalog-at-timestamp.md`, and
  `rs/hang/src/catalog/consumer.rs` returns each catalog with its timestamp.
  A quest file that moved, or went away without its work landing, does not
  count.

## Required

- [The studio is caught up to moq's main](/quest/m0/pin-main.md) - moq's breaking changes since moq-dev/moq#8272, so this pin takes only its own
