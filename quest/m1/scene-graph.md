# [M] Scene graph view

## Goal

A Graph panel shows the scene graph: picture and audio scenes as nodes, the
sources and scenes each one draws or sends to as edges, with each sound's
volume, pan, and a live waveform on its node. Edges and node settings can be
edited there, as in the Scenes and Audio panels.

## Plan

Decided 2026-11-05 with the maintainer (/quest-plan): the live controls stay
a mixer, as pro switchers keep them, and this view is for building and
inspecting routing, as node tools (TouchDesigner, Fusion) do. It reads and
edits the same data, so it adds no second model.
