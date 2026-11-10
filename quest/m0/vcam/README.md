# Virtual camera

## Goal

The studio exposes the program or a published scene as a system camera, so video-call and
conferencing apps, which offer no ingest API, can use it. The studio has one
toggle for the program and one per published scene, and each OS backend lands on its own.

## Plan

Decided 2026-10-22:
- **Linux:** a PipeWire video sink, which PipeWire-aware apps see as a
  camera. v4l2loopback is only a fallback, because it is a kernel module that
  needs root.
- **Windows:** `MFCreateVirtualCamera` on Windows 11. The legacy DirectShow
  filter that Windows 10 needs is out of scope, because Windows 10 support
  ended in October 2025.
- **macOS:** a CoreMediaIO Camera Extension, which needs signing.
- **Order:** the first backend to land adds the studio toggle.

The Linux backend landed the toggles: **Virtual camera** in the Output panel,
one for the program and one for each published scene, not saved in the scene
file. Each OS backend is a `vcam::backend` in `rs/moq-studio`, fed tightly
packed RGBA frames of what its camera shows on a `vcam` thread only while an
app reads the camera, and names its camera after the scene it shows.

## Required

- [Windows virtual camera](/quest/m0/vcam/windows.md) - a Media Foundation virtual camera on Windows 11
- [macOS virtual camera](/quest/m0/vcam/macos.md) - a CoreMediaIO Camera Extension inside the app bundle
