# [L] Chromium's sandbox for browser sources

## Goal

A browser source's renderer runs inside Chromium's sandbox on every
platform browser sources run on, so a hostile page, or an overlay site that
gets compromised, cannot read the creator's files, keys, or stream credentials
through a renderer exploit. The helper passes no `--no-sandbox`.

## Plan

Today every platform starts CEF with `--no-sandbox`
(`rs/moq-scene/src/browser/host.rs`), and the README warns to add only pages
you trust.

- **Linux:** Chromium's namespace sandbox needs unprivileged user namespaces,
  which some distributions restrict (Ubuntu's AppArmor rule since 24.04);
  the setuid `chrome-sandbox` fallback needs root to install. Recommendation:
  use the namespace sandbox, and when the kernel refuses it, fail the browser
  source loudly with what to enable rather than run unsandboxed.
- **macOS:** link CEF's `cef_sandbox` static library into
  `moq-browser-helper` and enter the Seatbelt sandbox first thing in its
  `main`, as CEF's sample helpers do. Check the renderer's and GPU helper's
  entitlements in `packaging/macos/sign.sh` still suffice.
- **Windows:** CEF's sandbox needs its `cef_sandbox.lib` linked into the
  executable and `bootstrap.exe`'s layout; land with or after
  [Browser sources on Windows](/quest/m0/browser-source/windows.md).
- A test per platform proves a page's renderer cannot open a file outside its
  profile.

## Related

- [Browser sources on macOS and Windows](/quest/m0/browser-source/README.md) - the browser sources this sandboxes
