# [XS] The media picker takes only what plays

## Goal

The quick start's **Media** card opens a file dialog that shows only the
images and videos the studio can play, and a file that still fails to open
says why where the streamer is looking, not behind the cards.

## Plan

Found 2026-11-25 trying the first launch: the dialog accepted a PDF, and its
error drew behind the quick start's cards. `media_dialog` in
`rs/moq-studio/src/ui/start.rs` (from
[moq-dev/studio#237](https://github.com/moq-dev/studio/pull/237)) sets no
filter. Add one filter, named for what it holds, listing the extensions the
image and video sources open; take them from where those sources decide, so
the two cannot drift. Draw the card's fault above the cards, or on the
picture, as Add puts its errors above its cards.
