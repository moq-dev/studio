# [XS] The join page is live

## Goal

`https://join.studio.moq.dev/` serves the join page (`js/join`) over a valid
certificate, so `--join-page` works without being passed.

## Plan

Done 2026-11-06 (maintainer and agent, /quest-unblock): Pages builds from
GitHub Actions, the `github-pages` environment deploys only from `main`, a
ruleset on `main` takes only PRs that pass `Check` and refuses force pushes
and deletion, Cloudflare has `join.studio` as a DNS-only CNAME to
`moq-dev.github.io`, Pages has `join.studio.moq.dev` as its custom domain, and
the Join page workflow deployed. `http://join.studio.moq.dev/` answers 200.

Left for an agent: once GitHub issues the certificate
(`gh api repos/moq-dev/studio/pages` shows `https_certificate.state` as
`approved`), enforce HTTPS (`gh api -X PUT repos/moq-dev/studio/pages -F
https_enforced=true`), check that `curl -sI https://join.studio.moq.dev/`
answers 200 and the page loads, drop the README's note that the default is
not live yet, and delete this quest.
