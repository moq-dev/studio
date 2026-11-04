# [XS] The program encoder drains

## Goal

The program's encoder drains its buffered pictures before it stops at exit
or when a new canvas ends its rendition, and before the studio publishes the
gap marker of an on-demand pause, so a
pipelined hardware encoder never loses the program's last frames.

## Plan

Source encoders already drain before they are replaced or their track ends,
and a failed drain aborts the track
([#3](https://github.com/moq-dev/studio/pull/3)). Give the program the same
behaviour through the same path rather than a second one. openh264 buffers
nothing, so prove it on an encoder that does, or with a test double that
holds frames.
