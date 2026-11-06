# [XS] The persistable portal restore token is pinned

## Goal

The `moq` submodule includes moq-video's persistable xdg-desktop-portal
restore token, from [Linux capture parity](https://github.com/moq-dev/moq/blob/main/quest/m3/capture-linux.md).

Check: that quest's restore-token bullet has landed on moq's `main` (the
quest file is gone, or no longer lists it). Advance it with
`just upgrade-moq` in its own PR, fixing whatever the new API breaks here.
