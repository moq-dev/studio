# [S] A shots test's process exits cleanly under load

## Goal

A `just shots` test process that passed never crashes on its way out, even on
a loaded host, fixed at the root cause in the studio's teardown, wgpu, or
lavapipe (the last two upstream). No retry and no skipped teardown. Under
nextest a crash after a pass still fails the run.

## Plan

What is known: while
[shots-scene-remove-flake](https://github.com/moq-dev/studio/pull/271) was
being reproduced, `shots::a_scene_removes_from_its_pad` printed `test result:
ok` and then died with SIGSEGV in 3 of 384 runs. That was 96 copies at once on
a 24-core host. In 48 runs at one copy per core it never crashed. The other
shots tests share the teardown (`quit`, then dropping the harness, the GPU,
and the runtime), so expect them to crash the same way.

Start by catching a core (`coredumpctl`, or `ulimit -c` under the stress loop
that PR describes) and reading its backtrace. If the crash is in the studio,
fix the order things drop in. If it is upstream, file it there and add a
`Waiting` item.
