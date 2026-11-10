# [L] Opening an OBS scene collection

## Goal

The show library opens an OBS scene collection and makes a show from it:
the same scenes, sources, positions, crops, order, visibility, volumes,
mutes, and sync offsets, with the filters and transitions the studio has
equivalents for. A report lists what was carried over, what was
approximated, and what was left out and why.

## Plan

The studio is for creators who rely on OBS today, and nobody rebuilds
twenty scenes to try a new tool.

- The converter is UI-free, in moq-scene, from OBS's collection JSON to a
  `Collection`, tested on collections kept in the repository.
- It maps what has an equivalent: capture, image, media, color, browser, and
  scene sources; text to an overlay; chroma key, color correction, LUT, crop,
  and mask filters; fade, cut, and stinger transitions.
- A device rarely keeps its id between the two apps, so a device source it
  cannot match arrives unpicked, and Add shows it as missing with its old
  name until one is chosen.
- Anything else is named in the report rather than dropped silently: plugin
  sources and filters, scripts, and audio monitoring that has no audio scene
  to map to.
- It reads the file and never writes OBS's.

## Waiting

- Maintainer: gather real OBS collections to test against, from your own
  setups and from creators willing to share one, and put them where the
  quest can read them. An agent can write synthetic ones but not the mess
  real ones hold.

## Required

- [Going live from inside the app](/quest/m0/ux/go-live.md) - the show library it opens from
- [Adding a source by picking a picture](/quest/m0/ux/add.md) - where a missing device is picked again
