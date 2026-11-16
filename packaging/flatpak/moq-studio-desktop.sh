#!/bin/sh
# The menu entry's launch, which passes no arguments: the studio's default
# scene collection in its data directory, the one `flatpak run` opens too, so
# the first launch runs guided setup, with recordings in the Videos folder the
# desktop names.
set -eu
exec moq-studio --recordings "$(xdg-user-dir VIDEOS)" "$@"
