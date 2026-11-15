# [S] Pictures on REVIEW's recording and clip cards

## Goal

Each finished recording's and clip's card on the REVIEW page of
[`mock/review.html`](/quest/m0/ux/mock/review.html) shows the file's first
picture, where it shows a placeholder tile today. The rest of the page is
built.

## Plan

- A finished recording or clip is a regular MP4, which moq-mux does not read
  yet. The picture is its first video sample, decoded small once per file in
  the shelf's background listing (`rs/moq-studio/src/shelf.rs`), as the
  strip decodes keyframes (`rs/moq-studio/src/past.rs`). A recording under way
  already shows the live picture.

Human check: scrubbing and picking a stretch by feel, on a show with a relay.

## Waiting

- Maintainer: how a card gets its picture. (a) Pull moq-dev/moq's
  [regular MP4 import](https://github.com/moq-dev/moq/blob/main/quest/m3/mp4-import.md)
  forward from m3 and decode through it (recommended: demuxing belongs in moq,
  and the media source needs the same import); (b) read the first sample here
  with `mp4-atom`, about 60 lines the import later replaces; (c) keep the
  placeholder tile and drop pictures from the mock-up.

## Related

- [Markers and clips](/quest/m2/markers.md) - adds markers to the strip and chapters to a recording
