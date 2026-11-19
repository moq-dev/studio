#!/bin/sh
# The menu entry's launch, which passes no arguments: the library of recent
# shows, and recordings in the Videos folder rather than beside each show.
set -eu
exec moq-studio --recordings "$(xdg-user-dir VIDEOS)" "$@"
