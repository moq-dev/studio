# [XS] moq-video's browser backends are pinned

## Goal

The `moq` submodule includes moq-video's browser backends (getUserMedia and
getDisplayMedia capture, WebCodecs, and VideoFrame import into wgpu) and the
wasm32 builds of hang and moq-mux, from
[moq-video in the browser](https://github.com/moq-dev/moq/blob/main/quest/m5/video-web.md).

Check: that quest's PR has merged on moq's `main`, deleting its file, and
moq-video's browser backends are there. A quest file that moved, or went away
without its work landing, does not count. Advance it with `just upgrade-moq`
in its own PR, fixing whatever the new API breaks here.
