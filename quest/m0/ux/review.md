# [M] The REVIEW page

## Goal

The REVIEW page of [`mock/review.html`](/quest/m0/ux/mock/review.html)
replaces the Replay panel and the recording half of the Output panel: the
last 30 seconds as a strip to pick a stretch from, what to replay (the
program or a source), the speed, and Replay and Save clip; beside it, this
show's recordings and clips as cards with a picture, a length, and a size,
one that was repaired after a crash marked as such, and a button that opens
their folder.

The title bar's record button starts and stops a recording from any page and
shows its clock while recording.

## Plan

Recording and repair behave as `README.md` has them; this is their place and
look. Recommended, not decided: Replay and Save clip take any stretch of the
last 30 seconds, picked with two handles, where today they take the last N
seconds up to the click. The track already holds the 30 seconds. The
alternative pins the right handle to now. The strip's pictures are frames the program's track
already keeps for late viewers, decoded at a low rate while the page is
shown, and nothing records in the background for it.

- The large picture is the frame under the scrub head, so the choice is
  seen before it is replayed. The program's card keeps its ON AIR tag, so the
  live picture never leaves the screen.
- A replay that cannot play says why where the button is: off air with no
  relay, a source with no past, a speed that drops the sound.
- A recording's card comes from the files in the recordings folder that
  carry this show's name. Reading their length and a first picture must not
  hold up the page.
- Which picture and audio scene a recording holds are Settings, Recording.

[Markers and clips](/quest/m2/markers.md) later adds markers to the strip
and chapters to a recording.

Human check: scrubbing and picking a stretch by feel.

## Required

- [One live picture](/quest/m0/ux/live.md) - the page and the record button
