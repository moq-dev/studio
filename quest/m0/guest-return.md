# [M] Guest return feed

## Goal

Guests in the studio's room see the program and hear a mix-minus, the program
audio without their own voice, so they can follow the show without hearing
themselves back.
The join page plays it.

## Plan

Decided 2026-11-02 (maintainer): guests get a mix-minus, not the full program,
and video and audio land together. Publish into the
room the studio already owns, under names the room's token scope allows, and
keep it free of the studio's own credentials. A guest's token subscribes only
to its own renewals (`.renew/<identity>/<generation>`), so it must also grant the feed's
paths, still leaving other guests' renewals out. One mix-minus per guest grows
with the guest count, so bound it by design (the guest roster is already
bounded to guests the studio signed for). Raised on
[#10](https://github.com/moq-dev/studio/pull/10).

Audio mixes landed in [#32](https://github.com/moq-dev/studio/pull/32): build
each guest's feed with moq-audio's `bus::Mix::minus` from the mix on program
(`Sources::mix`), taking the guest's own strip out, which costs one
subtraction per block rather than a mix per guest.
