# [XS] moq-audio's filter chain is pinned

## Goal

The `moq` submodule includes moq-audio's per-source filter chain and plugin
host, from [moq-audio filter chain](https://github.com/moq-dev/moq/blob/main/quest/m5/audio-filters.md).

Check: that quest's PR has merged on moq's `main`, deleting its file, and
moq-audio's filter chain is there. A quest file that moved, or went away
without its work landing, does not count. Advance it with `just upgrade-moq`
in its own PR, fixing whatever the new API breaks here.
