# [XS] In-band token renewal is released

## Goal

A published `@moq/net` presents a fresh token on a live session
(`connection.auth.add(token)`, its `Auth` export), and a published
`moq-relay` renews grants in band, over a moq-lite version that is not a wip
wire. moq's `main` has both today; its `release` branch, which npm and
crates.io publish, has neither, and `main` reaches `release` only at
[moq's next breaking cut](https://github.com/moq-dev/moq/blob/main/quest/m2/release/cut.md),
which also finalizes moq-lite-07.

Check: `npm view @moq/net version` is at or past that cut, and the package's
`index.d.ts` exports `Auth`. Advance it by bumping `js/join`'s `@moq/*`
dependencies to that release in its own PR, fixing whatever the new API
breaks.
