# [M] Linux virtual camera

## Goal

The studio publishes the program or a published scene as a PipeWire video node that
PipeWire-aware apps (browsers and conferencing apps) list as a camera. It
falls back to an existing v4l2loopback device when one is present.
The studio's Devices panel leaves out its own virtual camera, so picking it
cannot feed the program back into itself.
