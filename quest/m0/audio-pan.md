# [S] Pan each sound

## Goal

Each sound's strip in the Audio panel, and its node in the Graph panel, has
a pan beside its fader, saved as `pan` in `[audio.<source>]` and applied
wherever the sound plays, in every audio scene holding it, as gain, mute, and
delay are. `README.md` documents it.

## Plan

Decided 2026-11-07 with the maintainer, on
[moq-dev/studio#77](https://github.com/moq-dev/studio/pull/77), which landed
the rest of audio scenes:

- ✅ Pan is a property of the source's bus strip, like its gain, set in
  moq-audio; the studio only sets it and saves it. Rejected: a pan per audio
  scene a sound is sent to.
- ✅ Settled moq-side in
  [Pan a bus source](https://github.com/moq-dev/moq/blob/main/quest/m3/audio-strip-pan.md):
  a constant-power law, -3 dB at center; a stereo source is balanced, the
  far side attenuated and each channel kept where it is; a bus that is not
  stereo refuses a pan other than center.
- Changing pan never drops or repeats audio, ramped across a block as gain
  is.
- Both panels draw a sound's mute, gain, and delay with `strip_controls` in
  `rs/moq-studio/src/ui.rs`; a pan added there shows in both.

## Required

- [moq-audio's strip pan is pinned](/quest/m0/pin-audio-pan.md) - the `Strip` pan the studio sets
