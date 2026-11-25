Repository-specific review rules for `studio` (extends Quest's shared review rules, read from the base branch).

# Checks

- **Layer**: does logic belong in `moq-scene` vs `moq-studio`, or upstream in `moq-dev/moq` (capture, codecs, audio, catalog, transport)? Does a package boundary leak?
- **User-facing surface**: would we support this flag, panel, or scene-file shape for years? Does visible UI earn its place for most streamers rather than belonging under `Settings > Mods`, and is `README.md` + `just shots` updated?
- **Performance**: are frame-time / GPU-copy claims measured under a real compositor (not Xvfb)?

# Not findings

- A compatibility shim or migration for something no release has shipped; Studio is unreleased.
