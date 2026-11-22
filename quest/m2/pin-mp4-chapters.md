# [XS] moq-mux's MP4 chapters are pinned

## Goal

The `moq` submodule includes a way to give a recording that moq-mux's
`container::mp4` finishes, by `Export::finish` or `repair`, chapters: each a
moment on the recorded broadcast's timeline and a title, written into the
regular `moov`.

## Plan

moq-mux is where this belongs: it alone knows where the movie starts on the
broadcast's timeline, and on moq's `main` its `Finish` is opaque, so the
studio cannot edit the `moov` itself. mp4-atom 0.17, which moq-mux already
uses, encodes the Nero `chpl` box that ffmpeg, mpv, and VLC read.

moq-mux's regular MP4 finish writes chapters, as `chpl` and a QuickTime
chapter track, on moq's `main` since
[moq-dev/moq#9284](https://github.com/moq-dev/moq/pull/9284). Advance it with `just upgrade-moq` in its own PR,
fixing whatever the new API breaks here.

## Required

- [The studio catches up to moq's main](/quest/m0/pin-moq-tip.md) - absorbs the tip's other breaks, so this pin moves only by chapters
