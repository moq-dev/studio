# [S] Health grades source tracks

## Goal

The Health panel grades each published source track the way it grades the
program's: sent rate, viewers reporting, and the worst grade and latency they
report, so a source someone watches directly is covered too.

## Plan

Source tracks are listed in their own nested catalogs under the studio's
catalog (see `README.md`, Source tracks), and the health sampler reads only
the root renditions today. Viewers report per rendition through `.echo`, so
the per-track table and its causes extend to the source renditions rather than
adding a second path. Keep the table readable when there are many sources, for
example grouped by source. Raised on
[#3](https://github.com/moq-dev/studio/pull/3).

## Related

- [Source tracks](/quest/m0/compose/sources.md) - the audio source tracks this
  grades once they land
