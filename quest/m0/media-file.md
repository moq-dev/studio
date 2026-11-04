# [M] Media file source

## Goal

A source in `moq-studio` plays a local video or audio file, such as an intro
clip or background music: looping, or from the start each time something asks,
with its sound joining the mixes like any other source.

## Plan

Decided 2026-11-04 (maintainer): planned so stinger transitions have a clip
that starts on demand. Demux and decode through the moq crates the studio
already uses (moq-mux import, moq-video and moq-audio decode) rather than a
new media stack; anything they lack lands in moq first, with a moq-side test,
then a pin. Start with the container and codecs those crates already read, and
refuse anything else at setup with the reason. The file path is a local path
in the scene file, like an image source's. The web studio has no local paths:
there a file comes from a browser file picker for the session, and resolving
scene-file paths in the browser belongs to the [web studio](/quest/m0/web.md),
as it does for image sources.

A file is played on the program clock. Settings: loop, or play once and hold
the last frame; and whether going on program restarts it. A restart on demand
(the stinger's need) is an API on the source, not only a setting. A file that
ends or fails reads as ended or failed in the Sources panel and Health.

## Related

- [Stinger transitions](/quest/m0/stinger.md) - the first consumer of a clip that starts on demand
