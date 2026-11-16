# [XS] moq-audio's output kind is pinned

## Goal

The `moq` submodule includes an output's kind on
`moq_audio::playback::Device`: whether the system reports it as headphones
or a headset, as speakers, or does not say.

## Plan

cpal 0.18, which moq-audio already uses, reports it as
`DeviceDescription::device_type` on PipeWire and WASAPI, and as `Unknown`
elsewhere. `playback::list` reads the description for the name already, so
the change upstream is a field beside `name`, mapped from cpal's type so
cpal stays out of moq-audio's API, with a test of the mapping.

Once the wait clears, advance it with `just upgrade-moq` in its own PR,
fixing whatever the new API breaks here.

## Waiting

- moq-dev/moq: `moq_audio::playback::Device` on moq's `main` reports an
  output's kind. Check `rs/moq-audio/src/playback/device.rs` there. The
  change has no quest in moq's tree yet; it was found by
  [moq-dev/studio#178](https://github.com/moq-dev/studio/pull/178) and is
  listed in its Follow-ups for `/quest-plan` there.
