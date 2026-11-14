# [S] The browser helper reaches no Google service again

## Goal

`the_helper_requests_only_its_pages` (`rs/moq-studio/tests/browser.rs`), run
by `just render-sw`, passes again: a browser source's helper opens no
connection a page did not ask for.

## Plan

Found by [moq-dev/studio#163](https://github.com/moq-dev/studio/pull/163)
on 2026-11-14. On `main` (`5a74c18`), with the CEF runtime at
`cef-156.0.15` from the dev shell's `CEF_PATH`, the test fails every run:
the helper sends `CONNECT clients2.google.com:443` twice. The switches in
`rs/moq-scene/src/browser/host.rs` already point the component updater at
`data:` and disable the features that phoned home when the test was written,
so something else in this CEF release reaches `clients2.google.com`.

Find the feature or service behind the request, such as with
`--log-net-log` or by bisecting the `--disable-features` list, and turn it
off with a switch the way `host.rs` turns off the others. Never loosen the
test's assertion.
