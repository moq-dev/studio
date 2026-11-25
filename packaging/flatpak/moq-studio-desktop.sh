#!/bin/sh
# The menu entry's launch, which passes no arguments: the last show, and
# recordings in the Videos folder rather than beside each show, including
# shows made before new shows saved that folder themselves.
set -eu
exec moq-studio --recordings "$(xdg-user-dir VIDEOS)" "$@"
