# [XS] Browser sources paint on the GPU under Mesa AMD Wayland

## Goal

Browser sources copied on the GPU paint on an AMD GPU's Mesa driver under
GNOME on Wayland, as they do on Intel and NVIDIA.

## Plan

From [moq-dev/studio#111](https://github.com/moq-dev/studio/pull/111), which
fixed Intel: Mesa there picked a compressed, two-plane buffer layout
(`INTEL_Y_TILED_CCS`), and Chromium's frame capturer drops multi-plane
buffers, so the browser helper now starts with `INTEL_DEBUG=noccs`
(`rs/moq-scene/src/browser/run.rs`). AMD's compression (DCC) can add planes
the same way. Run `binary(browser)` on an AMD Mesa host, and only if it fails
the same way, add `AMD_DEBUG=nodcc` beside `noccs`.

Planned 2026-11-08 (maintainer, /quest-plan): ✅ m2 (rejected: m0; dropping
it), since no user has reported AMD failing. It waited on a host then.

Decided 2026-11-09 (maintainer, /quest-audit): ✅ agents may run the check on
the host named `desktop`, in its live GNOME Wayland session (rejected: wait
for a dedicated host). It has an RX 9070 on radeonsi. It also has an Intel
integrated GPU (`lspci`), so the run confirms the helper paints on the AMD
one. The check opens browser windows in the maintainer's session for a few
minutes.
