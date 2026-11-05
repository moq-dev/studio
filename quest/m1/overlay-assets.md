# [S] Overlay assets

## Goal

An overlay uses its own fonts and images, such as a logo or a brand face,
given as `data:` URLs or as files beside the scene file. Any other URL is
refused with an error naming it.

## Plan

From [moq-dev/studio#34](https://github.com/moq-dev/studio/pull/34).

Decided 2026-11-04 with the maintainer (/quest-plan):

- ✅ **Local files and `data:` URLs only.** With no network, what goes on air
  is deterministic and private, and an asset can't fail or change mid-show.
  Rejected: also fetching over https.

Implementation:

- Give Blitz a `NetProvider` that serves only `data:` URLs and files under
  the scene file's directory, rejecting `..` escapes and symlinks out of it.
- Widen `overlay::check` (`rs/moq-scene/src/overlay.rs`) to accept
  `@font-face` and `<img>` / `background-image` that resolve that way.
- Load assets before the overlay first paints, so its first frame on program
  has them.
- `README.md` documents where assets go.
