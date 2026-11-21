# [S] Nightly macOS check passes again

## Goal

The nightly workflow's macOS job, `just ci` on `macos-latest`, passes on
`main`, as it did on 2026-11-18.

## Plan

It has failed since 2026-11-19
([run 44766089244](https://github.com/moq-dev/studio/actions/runs/44766089244))
with two compile errors no Linux check sees:

- `rs/moq-scene/src/browser/mach.rs:289`, a test: no method `id` on
  `CFRetained<IOSurfaceRef>`. Likely a feature of `objc2-io-surface`
  (`IOSurfaceTypes`, which holds `IOSurfaceID`) that something else in the
  graph used to turn on; find what changed in `Cargo.lock` between the two
  runs and name the feature in `rs/moq-scene/Cargo.toml`.
- `rs/moq-studio/src/controls.rs`: `Fixed::id` and `Action::id` are dead code
  on macOS, under `-D warnings`. Give them their macOS consumer, or gate them
  on the platforms that read them.

Fix on a Mac or by pushing and reading the nightly run
(`gh workflow run nightly.yml --ref <branch>`); a pull request runs no macOS
`just ci`.

## Hosts

- macos - the errors compile only there
