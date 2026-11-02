# Virtual camera

## Goal

The studio exposes a canvas's program as a system camera, so video-call and
conferencing apps, which offer no ingest API, can use it. The studio has one
toggle per canvas, and each OS backend lands on its own.

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

The Linux backend landed the toggle: **Virtual camera** in the Output panel,
for the one canvas, not saved in the scene file. Each OS backend is a
`vcam::backend` in `rs/moq-studio`, fed tightly packed RGBA program frames on
the `vcam` thread only while an app reads the camera. With
[multiple canvases](/quest/m0/multi-canvas.md), the toggle moves to each
canvas's outputs.

## Required

- [Windows virtual camera](/quest/m0/vcam/windows.md) - a Media Foundation virtual camera on Windows 11
- [macOS virtual camera](/quest/m0/vcam/macos.md) - a CoreMediaIO Camera Extension inside the app bundle
