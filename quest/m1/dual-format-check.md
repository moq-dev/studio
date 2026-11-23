# [XS] Watch one show on a phone and a desk

## Goal

The maintainer has watched one show going out in both formats on a phone and
on a desktop at once, and each reads as made for its screen: the portrait
picture's main picture is cropped around what matters, the camera stacked
under it is not cut off, and overlays sit where a phone viewer sees them. If
it does not, how turning **Both at once** on places the portrait arrangement
changes until it does.

## Plan

[Landscape and portrait at once](https://github.com/moq-dev/studio/pull/200)
draws, encodes, and publishes the portrait picture at `<broadcast>/portrait`,
and its GPU and engine tests check the frames and the broadcast. Whether the
derived arrangement reads well on a real phone, and whether a viewer's player
picks the broadcast for its screen, are judgments no test makes.

## Waiting

- Maintainer: switch on **Both formats** in Settings, Mods, then **Both at once** in Settings, Show, on a show with a
  game or a screen as its main picture and a camera, go live to a relay,
  and watch `<broadcast>` on a desktop and `<broadcast>/portrait` on a phone.
  Done when both read as intended, or the change the derivation needs is
  written into this Plan.
