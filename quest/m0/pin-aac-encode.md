# [XS] Platform AAC encoders are pinned

## Goal

The `moq` submodule includes moq-audio's platform AAC encoders, from
[AudioToolbox AAC encode](https://github.com/moq-dev/moq/blob/main/quest/m2/audio-encode-audiotoolbox.md) on macOS and
[Media Foundation AAC encode](https://github.com/moq-dev/moq/blob/main/quest/m2/audio-encode-mediafoundation.md) on
Windows, which destinations whose ingest requires AAC use.

Check: both quest files are gone from moq's `main`. Advance it with
`just upgrade-moq` in its own PR, fixing whatever the new API breaks here.
