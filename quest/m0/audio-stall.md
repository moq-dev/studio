# [S] Audio sources report stalls

## Goal

A live audio source (a microphone or system audio) that stops delivering
samples reads "stalled" in the Sources panel and Health, as a live camera does
after a second without a picture.

## Plan

The audio bus reports no last-delivery time per source, so the stall check
video sources have cannot see it. The studio opens each capture as a
`capture::Producer`, so read delivery from a consumer of its own rather than
adding a side channel in the bus. Raised on
[#5](https://github.com/moq-dev/studio/pull/5).
