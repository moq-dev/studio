# [S] Clean quit

## Goal

Quitting the studio delivers everything it published before the process
exits: the program's video and audio and each source's own track end
cleanly, and the relay session closes gracefully, so a viewer or a recording
on the relay receives the tail of the broadcast instead of a cut.
A worker that fails to start rolls back what it already started, rather
than leaving threads or tracks running.

## Plan

From [moq-dev/studio#23](https://github.com/moq-dev/studio/pull/23) and
[moq-dev/studio#22](https://github.com/moq-dev/studio/pull/22).

- `Engine::stop` (`rs/moq-studio/src/engine.rs`) drains and finishes the
  program video, but `main` then returns and drops the runtime, so the tail
  may never reach the relay.
- `moq_tokio::Connection::close` drains, bounded at one second, once every
  track is finished: program audio, and each source's track. Source threads
  are not stopped at exit today, so stop them and finish their tracks first.
- A worker that fails partway through starting (the stop signal raised on
  #10 and #22) stops the parts it already started before reporting the
  error.
- Test on a paused `kio::clock`: quit while live to a local relay, and a
  subscriber receives the last group of every track and then the end.
