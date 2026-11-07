# [XS] Overlays drop their Blitz panic guard

## Goal

`overlay::check` (`rs/moq-scene/src/overlay.rs`) parses an overlay's HTML
without `catch_unwind`, because the Blitz it builds on reports an
unresolvable `src` or `href` instead of panicking.

## Plan

From [moq-dev/studio#62](https://github.com/moq-dev/studio/pull/62); picked
2026-11-06 (maintainer, /quest-plan), m2 since the guard works meanwhile.

[DioxusLabs/blitz#1374](https://github.com/DioxusLabs/blitz/pull/1374)
(merged 2026-11-03) skips a resource whose URL cannot resolve. Bump the Blitz
crates to the release carrying it, drop `catch_unwind`, and keep the error
`check` gives for such a URL, now found by walking the document.

## Waiting

- DioxusLabs/blitz: a release after 0.3.0-beta.4 with #1374. Check: the
  newest `blitz-dom` on crates.io is past 0.3.0-beta.4, and its
  `Document::resolve_url` returns an `Option`.

## Related

- [DioxusLabs/blitz#1455](https://github.com/DioxusLabs/blitz/issues/1455) - root and `<body>` background images don't reach the canvas; overlays put an image on a sized element until then
