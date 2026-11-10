# [L] The MIX page

## Goal

The MIX page of [`mock/mix.html`](/quest/m0/ux/mock/mix.html) replaces the
Audio panel:

- One strip per sound, with its icon and name, a tall meter, a fader, and a
  mute, and the audio scenes as master strips beside them.
- Under each strip, where it is heard: one dot per audio scene. The audio
  scene the stream carries is marked as such, and any can be listened to
  with a headphone button.
- A strip opens to its pan, its delay, and its filters.
- The same sends as a matrix, for someone who thinks in routing.

The LIVE page's sound strip is the same meters and mutes for the sounds the
stream carries.

## Plan

Nothing about mixing changes: gain, pan, mute, delay, sends, the audio scene
on program, the recorded one, and Listen are as `README.md`'s Audio scenes
section has them. "Audio scene" stays the word, since it was chosen over
"mix" on 2026-11-06
([moq-dev/studio#77](https://github.com/moq-dev/studio/pull/77)).

- A fader is gain in decibels, with unity marked and a double click
  returning to it, as pan returns to center today.
- The meter keeps the peak tick and the red at full scale of
  `rs/moq-studio/src/ui/levels.rs`, drawn as segments.
- A sound heard twice by construction cannot happen here, since an audio
  scene plays a sound once however it reaches it. Show that: a dot is filled
  when the sound reaches the audio scene by any route, and hollow-ringed when
  it does so only through another audio scene.
- A microphone feeding the output it is heard on is the one loop the studio
  cannot prevent. Warn on Listen when the output is not headphones, where the
  system can tell.
- [Audio filters](/quest/m0/audio-filters.md) and
  [clap to measure](/quest/m0/sync-clap.md) add their controls to the opened
  strip; this quest leaves their places.

Human check: a real microphone and headphones, and whether the sends read at
a glance to someone who has never seen an audio scene.

## Required

- [One live picture](/quest/m0/ux/live.md) - the page and the sound strip

## Related

- [Audio filters](/quest/m0/audio-filters.md) - the filter chain an opened strip configures
- [Clap-to-measure sync delay](/quest/m0/sync-clap.md) - the button beside a strip's delay
