# [M] Linux Flatpak

## Goal

Every release ships a Flatpak of `moq-studio`, built in CI with the `release`
and `pipewire` features and verified to launch and list devices on Linux.

## Plan

The feature policy is the [epic](/quest/m0/packaging/README.md)'s.

Decided 2026-10-26 (maintainer): the Flatpak bundles libpipewire, so it adds
`pipewire`, which `release` leaves out, itself: inside the sandbox the camera
portal is the only way to a camera, and its PipeWire DMA-BUFs are what the
compositor imports without a copy.
