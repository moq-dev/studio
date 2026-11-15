# [S] Close the show and return to the library

## Goal

A broadcaster switches shows without quitting: the show's menu in the title
bar has Close show, which returns the window to YOUR SHOWS, the library of
recent shows. Today the library appears only when the studio starts with no
scene file, so changing shows means quitting and starting again.

## Plan

Raised by [moq-dev/studio#175](https://github.com/moq-dev/studio/pull/175),
which added the library (`rs/moq-studio/src/library.rs`) and the
`App::Library` state in `rs/moq-studio/src/main.rs`.

- Closing does what quitting does for the show: it flushes the autosave,
  saves the show's picture for its library card, records how long it last
  went live, finishes a recording, and closes the watch sessions, encoders,
  and capture. Then the window shows the library, its cards refreshed.
- While on air, Close show is disabled, its tooltip saying to end the stream
  first. Recommended over ending the stream as a side effect of closing (a
  menu click should not end a broadcast that END STREAM makes the
  broadcaster hold for) and over keeping the stream up across the switch (the
  stream belongs to the show being closed).
- Its README entry and a `just shots` view of the menu with the row.
