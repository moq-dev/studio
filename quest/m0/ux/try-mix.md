# [XS] Try the MIX page

## Goal

The maintainer tries the MIX page on their own machines and either signs it
off or files what they find as quests: what the epic's
[What a person must check](/quest/m0/ux/README.md) asks of it, which no
headless run exercises.

## Plan

The MIX page (its strips, the audio scenes as master strips, the opened
strip, and Routes) is on `main` (`cargo run -p moq-studio --features
capture`; Listen and its output picker are disabled without `capture`).
What to try, beside the [mock-up](/quest/m0/ux/mock/mix-mix.webp):

- **Real devices:** a real microphone and headphones. **Listen** on an audio
  scene, then pick the output from its **On …** tag; the meters moving with
  real sound, the peak held, red only at a clip.
- **Feel:** riding a fader with a mouse and a trackpad, whether the taper is
  fine enough near unity, a double click returning to it; mute and unmute
  during a show; opening a strip while the others narrow.
- **Readability:** whether the send dots (filled, ringed when heard only
  through another audio scene, faint when not heard) read at a glance to
  someone who has never seen an audio scene. If they do not, **Routes** is
  the fallback to point people at, and the legend's wording is the cheapest
  thing to change.

An agent then turns each finding into a quest, or an edit to the quest that
owns that page, and deletes this one.

## Waiting

- Maintainer: try the MIX page as the Plan lists and report what to change,
  or that nothing needs to. An agent cannot judge feel or taste, and the
  agent hosts have no microphone or headphones; a kittest run already covers
  the clicks (`shots::mix_sends_and_mutes`), and `just shots` the layout.
  Check: a comment or quest records the result.
