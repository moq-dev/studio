# [XS] The join page is live

## Goal

`https://join.studio.moq.dev/` serves the join page (`js/join`), so
`--join-page` works without being passed. The maintainer:

- enables GitHub Pages for moq-dev/studio with GitHub Actions as its source,
  and limits the `github-pages` environment's deployment branches to `main`
- points `join.studio.moq.dev` at Pages in DNS, sets it as the custom domain,
  and enforces HTTPS
- runs the Join page workflow (`.github/workflows/join.yml`) once

Check: `curl -sI https://join.studio.moq.dev/` answers 200 over a valid
certificate, and the page loads. Then drop the README's note that the
default is not live yet, and delete this quest.
