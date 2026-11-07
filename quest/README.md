# Studio questline

## Goal

Keep the repository's living work organized as visible, versioned quests,
grouped into milestones ordered by priority.

## Plan

The studio moved here from moq-dev/moq on 2026-11-02, with its quests. Its
epic became m0, keeping its priority order; split later milestones out of it
as the backlog grows.

Work that changes a moq crate is planned in
[moq's quest tree](https://github.com/moq-dev/moq/tree/main/quest) and lands
there first. A quest here that needs it waits on a pin quest, which
`just upgrade-moq` advances.

## Required

- [m0: the studio](/quest/m0/README.md) - the native and wasm studio for creators who rely on OBS today
- [m1: after the studio ships](/quest/m1/README.md) - viewers and a server compose a broadcast from its sources and a layout track, plus studio features held past m0
- [m2: later](/quest/m2/README.md) - work with no consumer yet
- [m3: someday](/quest/m3/README.md) - work no customer asks for, kept so its plans are not lost
