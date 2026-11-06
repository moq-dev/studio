# [XS] The persistable portal restore token is pinned

## Goal

The `moq` submodule includes moq-video's persistable xdg-desktop-portal
restore token, from [Linux capture parity](https://github.com/moq-dev/moq/blob/main/quest/m3/capture-linux.md).

Check: moq-video on moq's `main` exposes the persistable restore token (the
quest no longer lists it because it landed, not because it moved). Advance it with
`just upgrade-moq` in its own PR, fixing whatever the new API breaks here.
