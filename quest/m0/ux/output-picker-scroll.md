# [XS] The output picker scrolls

## Goal

The **Listen on** output picker (Listen's **On …** tag and a sound's
**Monitor in headphones**) stays inside the window and scrolls with the wheel
and a trackpad, however many outputs the system lists. On a Linux desktop
with PipeWire, 28 outputs overflowed it off screen with no way to reach the
rest.

## Plan

Found 2026-11-25 trying the first launch (GNOME Wayland). The list is
`Speakers::menu` in `rs/moq-studio/src/ui/mixer.rs`, which draws every
output with no scroll area; it came in with
[moq-dev/studio#178](https://github.com/moq-dev/studio/pull/178). Cap its
height to what fits under the button and put the list (not the **Listen on**
heading or **System default**) in a scroll area. A kittest view with many
outputs checks it.
