# [XS] Try the phone remote

## Goal

The maintainer drives a show from a real phone and either signs the remote
off or files what they find as quests: how it feels to use, which no
headless or desktop-browser run exercises.

## Plan

The remote is on `main` once
[moq-dev/studio#212](https://github.com/moq-dev/studio/pull/212) merges, and
the join page deploy serves it at `https://join.studio.moq.dev/remote.html`.
With a room set up in Settings, Guests (see the README's Guests and Phone
remote sections), pair a phone in Settings, Remote, and scan its QR code.
What to try, beside the [LIVE mock-up](/quest/m0/ux/mock/shell-live.webp):

- **Reach:** the phone on cellular rather than the studio's network, then
  back on Wi-Fi; whether the page reconnects on its own and says so.
- **Feel:** the pads, TAKE, AUTO, and the mutes under a thumb, with the
  phone locked and woken in between; whether a press ever seems lost or done
  twice; whether the time between a press and the program changing is short
  enough to cut on.
- **Readability:** the tally (ON AIR, NEXT, FIRST) and the frame in the
  accent while live, at arm's length.
- **Removal:** Remove in Settings, Remote, and how the page shows it.

An agent then turns each finding into a quest, or an edit to the quest that
owns the remote, and deletes this one.

## Waiting

- Maintainer: try the remote from a phone as the Plan lists and report what
  to change, or that nothing needs to. An agent has no phone to hold and
  cannot judge feel; a headless run already covers the protocol
  (`guests::remote::tests`), and the agent Chrome at phone size covered the
  page's clicks.
  Check: a comment or quest records the result.
