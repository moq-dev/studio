# [S] Linux virtual camera for each published scene

## Goal

Each published scene has a **Virtual camera** toggle of its own, as the
program does, and shows as its own PipeWire camera named after the scene, or
else its own v4l2loopback device. An app can then pick the program or any
published scene. The Devices panel leaves out every camera the studio
publishes, so picking one cannot feed it back into itself.

## Plan

The program's camera landed in
[moq-dev/studio#9](https://github.com/moq-dev/studio/pull/9): `vcam::Camera`
in `rs/moq-studio` reads the main canvas back through moq-video's
`render::readback` and reopens when the canvas changes size or frame rate.
Its PipeWire node name (`moq-studio`) and description ("MoQ Studio") are
fixed, so a camera per scene needs a name of its own, such as
`moq-studio-<scene>`.

## Required

- [Published scenes](/quest/m0/scenes/published.md) - any scene publishes as its own broadcast at its own size, replacing canvases
