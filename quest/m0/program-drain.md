# [XS] The program encoder drains

## Goal

The program's encoder drains its buffered pictures before it stops at exit
and before the studio publishes the gap marker of an on-demand pause, so a
pipelined hardware encoder never loses the program's last frames.

## Plan

Every rung of the program and of each source now encodes through
`moq_scene::ladder::Rendition`, which flushes before an on-demand pause's gap
marker and drains when dropped, aborting the track on a failed drain
([#21](https://github.com/moq-dev/studio/pull/21)). What remains is exit: the
process ends without dropping the program's encode threads, so give the engine
a shutdown that does. openh264 buffers nothing, so prove both on an encoder
that does, or with a test double that holds frames.
