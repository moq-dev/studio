# [S] Listen warns on speakers

## Goal

**Listen** on the MIX page warns, on the listened audio scene's strip and
before it plays, when the output it plays on is not headphones and the audio
scene carries a microphone: the one feedback loop the studio cannot prevent.
Where the system cannot tell what an output is, it says nothing.

## Plan

Split from the MIX page, which landed Listen's output picker on the listened
audio scene's strip (`Panels::listen_tag` in `rs/moq-studio/src/ui/mixer.rs`)
but cannot tell headphones from speakers yet.

- `mix::Speaker` carries the output's kind from
  `moq_audio::playback::Device`, and the system default's kind is the
  default device's.
- The warning is amber, as everything that needs attention but is not an
  error, and does not block Listen: a speaker in another room is fine.
- A headset counts as headphones.

## Required

- [moq-audio's output kind is pinned](/quest/m0/pin-output-kind.md) - whether an output is headphones, where the system can tell

## Related

- [Audio is layers](/quest/m0/ux/audio-layers.md) - removes the MIX page; Listen moves with the mixer into the Layers column
