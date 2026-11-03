# [S] Browser source popups

## Goal

In-page popups of a browser source, such as `<select>` dropdowns, draw on the
canvas and take input like the rest of the page, on both the GPU import and
the CPU fallback paths.

## Plan

CEF paints popup widgets separately (`PET_POPUP`) with their own rectangle,
and the helper drops them today; `README.md` states the limitation, and this
quest removes that note. `window.open` and `target="_blank"` stay cancelled:
those are new browsers, not widgets. Raised on
[#8](https://github.com/moq-dev/studio/pull/8).

## Related

- [Browser sources](/quest/m0/browser-source.md) - the macOS and Windows half
