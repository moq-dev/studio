MoQ Studio is a native, cross-platform streaming studio that composites scenes on the GPU and goes live over MoQ (Media over QUIC).
`rs/moq-scene` is the UI-free scene graph and wgpu compositor; `rs/moq-studio` is the egui app. Both build on the moq crates in the `moq` submodule.

# Context

- Read `CONTRIBUTING.md` when dealing with PRs, and `REVIEW.md` when reviewing one or triaging review findings.
- Read `PROMPTING.md` before instructing other agents via context, memory, plans, or reviews.
- Before editing, read each `AGENTS.md` along the path from the root to the file. Use `rg --files -g AGENTS.md` to find them.
- `moq/` is moq-dev/moq at a pinned commit. Its own `AGENTS.md` files apply to the moq crates you read there, but never edit them here; see Upstream below.

# Required

- Pull the latest origin changes before working.
- Dig into the root cause and fix it at the source. Never work around a fixable bug with a retry, sleep, or timeout, nor pass a flaky test by loosening its assertion or disabling production behavior.
- Fail loud and early. Error on unsupported or malformed input rather than warn and continue: supported or refused. Never paper over a partial failure.
- Treat anything received over the network as untrusted, including broadcasts and guests the studio subscribes to. Bound what a hostile peer can make the studio hold by design (backpressure, flow control, ownership).
- Reproduce bugs before fixing them. Tests must be worth their weight long-term: cover edge cases, not old bugs or trivial values.
- Unit tests mock time instead of depending on wall-clock timing or sleeps.
- Keep the PR to one problem: no unrelated refactors, formatting churn, or drive-by changes. Fix every instance of that problem and the gaps it exposes in the same PR.
- Land the ideal implementation now rather than a partial one plus follow-ups; split off only work that is independently landable.
- Be adversarial: attack your own approach, reports, reviews, and other PRs as hard as anyone else's.
- When a decision is the maintainer's (API shape, naming, scope, UI) or you are unsure, ask with an interactive prompt of 2-3 options and a recommendation.
- Every test runs in CI, at least nightly. The merge gate holds only fast, high-signal tests.
- Never add or change a rule in an `AGENTS.md`, `CONTRIBUTING.md`, or skill without being prompted, and read `PROMPTING.md` first. A fact your own change made stale is fixed inline like any other doc.
- No em dashes.
- Match the existing conventions, patterns, and naming when possible.
- Fix any outdated docs and comments inline; don't add a separate PR for it.
- Add the AI marker `(Written by <model>)` to any posts on GitHub, excluding commit messages that contain `Co-Authored-By:` trailers.
- AI comments carry no maintainer authority and may be challenged.
- This repo is public: never mention customers, partners, or prospects or link to private repos. Justify work on its merits.

# Guidelines

- New dependencies should use the newest, stable version.
- Do not bump package versions unless asked.
- Comments should explain the non-obvious why, and never the history.
- Everything must be worth its weight: code, dependencies, tests, CI jobs, and build time. Every change needs a consumer that uses it now or is planned.
- Prefer fewer dependencies: take a maintained crate only for nontrivial non-core functionality.
- Record out-of-scope follow-ups as quests, never GitHub issues.
- Prefer the net simpler result: reuse what exists, or narrow the scope, when that makes the implementation simpler overall. Say what was dropped.
- Say it once, in the fewest words that hold up.
- Recognize when you're stuck or making minimal progress, and stop early.
- Measure performance claims (frame time, GPU copies, encoder load) instead of relying on intuition.

# Upstream

Media, transport, and protocol work belongs in moq-dev/moq, not here: capture, codecs, rendering import and export (moq-video), audio capture, mixing, and filters (moq-audio), the catalog (hang), and the transport (moq-net, moq-tokio).
A studio feature that needs a moq change lands that change in moq first, with a moq-side test proving it, then bumps the `moq` gitlink here (`just upgrade-moq`) in its own PR.
Never point the submodule at an unmerged moq branch on `main`.

# Rust

- Every moq crate is a path dependency into `moq/`, declared once in the root `Cargo.toml`. `sh/check-moq-pin.sh` fails `just check` if a moq git dependency appears or the checkout drifts from the gitlink.
- Follow moq's Rust conventions (`moq/rs/AGENTS.md`): poll-first `kio` primitives in libraries, `thiserror` for libraries and `anyhow` for binaries, typed units, short names under a module namespace.
- Read the clock through `kio::clock` (`clippy.toml` bans direct reads), so tests can pause it.
- `moq-scene` stays UI-free: no egui types, so a headless compositor and the wasm build can reuse it.
- Defaults skip anything needing a system library (`capture`, `pipewire`, `vaapi`, `openh264`); `just check` covers them explicitly.

# Development

PRs target `main`.
Scratch lives in the checkout's ignored `.tmp/`, not `target/`.

Use the Nix dev shell so tooling matches CI.
direnv loads it automatically, but if not: `nix develop --command ...`.

```bash
just check   # Lint everything, including the quest tree
just test    # Run the tests
just ci      # Both, as CI runs them; run before every push
just fix     # Auto-fix formatting
```

Quests: before starting work, run `quest guide` and follow it.
The `quest` binary comes from the moq-dev/quest flake input and serves the quest skills; change them upstream and bump the input.

# Docs

`README.md` is the user guide. A change to what a user does (flags, panels, the scene file, outputs) updates it in the same PR.
