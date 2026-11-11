# [S] Guests are invited and placed from Add

## Goal

The Guests section of Add (the `guests` frame of
[`mock/add.html`](/quest/m0/ux/mock/add.html)) replaces the Guests panel:
Invite makes a link and shows its QR code, each guest who joined is a card
with their live picture, dragged onto the picture like any source, and a
guest's card offers Remove from scenes and New link. A failing token renewal
shows on the card.

Without a room configured, the section says what to set and opens that
Settings pane.

## Plan

The behavior is today's (`rs/moq-studio/src/guests.rs`): only the place
changes. A guest's sound still joins no audio scene until placed, so the card
says "not heard yet" until it is sent to the stream's mix, with the button
that does it.

## Required

- [Adding a source by picking a picture](/quest/m0/ux/add.md) - the page and the cards
