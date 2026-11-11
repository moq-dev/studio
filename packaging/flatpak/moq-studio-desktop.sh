#!/bin/sh
# The menu entry's launch, which passes no arguments: the scene collection in
# the app's own data directory, so the first launch runs guided setup, and
# recordings in the Videos folder rather than beside it.
set -eu
exec moq-studio --recordings "$(xdg-user-dir VIDEOS)" "$XDG_DATA_HOME/scenes.toml" "$@"
