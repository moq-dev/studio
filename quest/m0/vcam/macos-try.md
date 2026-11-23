# [S] The macOS virtual camera reaches apps on a Mac

## Goal

On a Mac running a signed release, turning a virtual camera on installs the
camera extension after one approval in System Settings, and FaceTime, Zoom,
and Chrome list "kixel.live Studio" and "kixel.live Studio: <scene>" and show
their pictures. Installing the next release replaces the extension without a
reboot. Whatever breaks on the way is fixed here.

## Plan

moq-dev/studio#217 added the extension without running it: installing a
system extension takes the release's Developer ID profile and a person's
approval, which no runner has. Check first what it assumed:

- The app reaches the extension's custom properties (`kcfg`, `krdg`) through
  `CMIOObjectGetPropertyData` and `CMIOObjectSetPropertyData`, and hears
  `krdg` change through a listener block.
- A camera's sink stream is the one whose `kCMIOStreamPropertyDirection` is 0.
- `consumeSampleBufferFromClient` waits for a picture; when it returns none,
  the extension looks again a frame later.
- The extension's team-prefixed app group needs no provisioning profile of
  its own under a Developer ID signature.
- Apps take 32-bit BGRA.

`log stream --predicate 'subsystem == "com.apple.cmio"'` and
`systemextensionsctl list` show what the system made of it.

## Hosts

- macos - approving the extension needs a Mac's GUI session

## Waiting

- Maintainer: on a Mac, install the [latest release](https://github.com/moq-dev/studio/releases/latest) (v0.0.1 or later) to Applications, turn on Settings > Stream > Virtual camera (Settings > Advanced in v0.0.1, which predates Mods), allow the extension in System Settings > General > Login Items & Extensions > Camera Extensions, turn the camera on again, and pick "kixel.live Studio" in FaceTime or Photo Booth; do the same for a published scene's camera, after switching on Settings > Mods > Published scenes in a release that has Mods, and report what each showed. Only a person can approve a system extension (System Settings asks for the user's password or Touch ID), and no CI runner can install one. Check: `systemextensionsctl list` shows `live.kixel.studio.camera` as `[activated enabled]`.
