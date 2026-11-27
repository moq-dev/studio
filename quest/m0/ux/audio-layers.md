# [L] Audio is layers

## Goal

Sound is layers. The Layers column is the mixer: each layer with sound shows
a live level meter, mute, and a volume slider in its row, and a sound-only
layer (a microphone, the computer's sound) can be added and deleted like any
other layer. A scene's sound is its layers' sounds.

- Every new show has a Microphone layer, and a Computer sound layer where the
  platform can capture it, each linked across every scene so it never cuts
  out on a scene switch. The quick start stays on an empty picture until the
  scene has a picture layer: sound layers do not count.
- The MIX page, audio scenes, and routes go away, and so does the sound strip
  at the bottom of the Studio page.
- A "Separate recording mix" mod adds per-layer Stream and Recording toggles
  to the inspector, for keeping something such as music out of the recording.

## Plan

Found trying Add on 2026-11-27 (maintainer, [Try Add](/quest/m1/try-add.md)):
a sound added from Add has no layer and cannot be deleted, and audio living
on its own in the mixer is confusing.

Decided 2026-11-27 (maintainer, /quest-plan):

- ✅ Combine Layers and the mixer in the Layers column (rejected: audio layers
  with the MIX page kept as a mod; deleting split mixes too).
- ✅ Volume stays per source, so one camera in two scenes keeps one level
  (proposed with the split-mix question, which the maintainer took).
- ✅ Split mixes survive only as the "Separate recording mix" mod's per-layer
  Stream and Recording toggles (rejected: dropping split mixes).
- ✅ A Microphone layer in every new show, and Computer sound where moq can
  capture it, linked across scenes.

Today: each source carries volume, mute, pan, and delay
(`rs/moq-scene/src/collection.rs`); audio scenes are sets of sources, and
adding a source with sound puts it in the program's audio scene
(`rs/moq-scene/src/edit.rs`). The bottom strip is `rs/moq-studio/src/ui/sound.rs`,
and MIX with Routes is `rs/moq-studio/src/ui/mixer.rs` behind the Audio
scenes & routes mod. Add's Audio tab adds a sound with no layer
(`rs/moq-studio/src/ui.rs`). The per-source gear (monitor, delay, pan,
filters) moves to the layer row or the inspector.

Recommendations:

- A scene file with audio scenes or routes loads as layers, its program audio
  scene's sources becoming sound layers in each scene that heard them, and
  routes to the recording becoming the mod's toggles. Say in the PR what
  cannot carry over.
- Draw the Layers column with meters in [`mock/overhaul.html`](/quest/m0/ux/mock/overhaul.html)
  first; keep the row height that [Layers and the inspector stay put](/quest/m0/ux/layers-polish.md)
  sets.
- Listen (monitoring a mix on headphones) moves with the mixer; carry
  [Listen warns on speakers](/quest/m0/ux/listen-speakers.md) to wherever it
  lands.
- Computer sound: macOS is offered today; Linux and Windows wait on moq's
  [capture-linux](https://github.com/moq-dev/moq/blob/main/quest/m3/capture-linux.md)
  and [capture-windows](https://github.com/moq-dev/moq/blob/main/quest/m3/capture-windows.md),
  and per-app sound on those and
  [capture-app-audio-macos](https://github.com/moq-dev/moq/blob/main/quest/m4/capture-app-audio-macos.md).
  Offer the layer only where it works.

Update the README's flags, panels, and scene file sections, and
[the epic's decisions](/quest/m0/ux/README.md) on the bottom dock and the
Audio Scenes & Routes mod.

## Required

- [Layers and the inspector stay put](/quest/m0/ux/layers-polish.md) - the row spacing the meters fit in
