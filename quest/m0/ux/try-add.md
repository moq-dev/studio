# [XS] Try Add

## Goal

The maintainer tries Add on their own machines and either signs it off or
files what they find as quests: what the epic's
[What a person must check](/quest/m0/ux/README.md) asks of it, which no
headless run exercises, and what [Try the Studio workspace](/quest/m0/ux/try-studio.md)
and [Try the first launch](/quest/m0/ux/try-first-run.md) left to it.

## Plan

Add is on `main` (`cargo run -p moq-studio --features capture,openh264`,
adding `pipewire` on a Wayland desktop for its screen picker): **Add a
layer**, `A`, the picture's right-click **Add a layer**, or the palette's
**Add a source** opens it over the stage, beside the
[mock-up](/quest/m0/ux/mock/overhaul-add.webp). What to try:

- **Real devices:** a webcam, a capture card, and a second display listed by
  name the moment Add opens, and a camera plugged in after launch showing the
  next time it opens, with no **Refresh**. Each card adds its device in one
  click and closes Add with the new layer selected.
- **Placement over a live screen:** a display added first fills the picture;
  a camera added over it lands as a corner picture-in-picture, the next one
  in the next free corner; a color goes behind everything. Whether those
  feel right, and dragging a card onto the picture, landing where it is let
  go.
- **Microphones:** the Audio tab's level meters moving with your voice, so
  the right input is obvious, and whether opening every listed input to
  meter it is acceptable on your machines (on Linux the same hardware lists
  once per sound host).
- **System dialogs:** the camera, microphone, and Screen Recording prompts on
  macOS (`tccutil reset Camera live.kixel.studio`, `Microphone`, and
  `ScreenCapture` first), the desktop's own picker from **Picked display** and
  **Picked window** on Wayland, and how a card that cannot be used says why
  and offers **Try again**.
- **Media & Web:** **Image or video file…** opening the system's file
  dialog, a pasted web address, **Text**, and **Color**.
- **The first launch:** from a cold launch (moving the app's storage aside as
  [Try the first launch](/quest/m0/ux/try-first-run.md) says), count the
  clicks to a live screen with a camera picture-in-picture, through the
  quick start and Add.

An agent then turns each finding into a quest, or an edit to the quest that
owns that part, and deletes this one.

- ✅ Keep this quest (maintainer, on
  [moq-dev/studio#262](https://github.com/moq-dev/studio/pull/262)).
- ✅ Placement in both pictures: where the portrait and landscape pictures
  disagree on the order, a new source goes in front as a picture-in-picture
  in both. Check it feels right with **Both at once** on.

## Waiting

- Maintainer (PC): try Add as the Plan lists and report what to change, or
  that nothing needs to. An agent cannot judge feel, and the agent hosts have
  no camera, microphone, capture card, or permission prompts; a kittest run
  already covers the clicks, the placement, and a drag onto the picture, and
  `just shots` the layout. Check: a comment or quest records the result.
