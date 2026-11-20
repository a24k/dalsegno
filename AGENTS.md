# Working on dalsegno

Read the docs before changing behaviour: [requirements](docs/requirements.md),
[roadmap](docs/roadmap.md), [decisions](docs/decisions.md) and [design](docs/design/). Work comes
from the issues of the current milestone.

## Rules

- **Public repository (D-19).** Never name the reference stream, its channel or artist; write "the
  reference stream" and use `Example TV` in examples. Fixtures, media, cookies, tokens, webhook
  URLs and real video IDs stay out of commits, PRs and issues. Fragment URLs carry the client
  address, so they are never printed, logged or stored.
- **English** for everything in Git and GitHub.
- **Crates depend downward only** ([architecture](docs/design/architecture.md#workspace)), and
  `cli` reaches `core`, `io` and `store` only through `app` (D-17).
- **Tests never reach YouTube.** Real fixtures load from `DALSEGNO_FIXTURES`, and tests that need
  them skip without it.
- **Every change goes through a PR**, which closes its issue with `Closes #N`.
- **Releases follow D-32.** The tag matches the version in `Cargo.toml`, and a milestone's run
  happens on a `-rc.N` first.

## Web UI

`web/` is a Svelte SPA (D-18) that the `server` crate embeds from `web/dist`. To work on it,
run `dalsegno serve` and then `npm run dev` in `web/`: Vite serves the pages and sends `/api`
to `127.0.0.1:8737`, or to `DALSEGNO_API`. A debug build reads `web/dist` from disk, so
`npm run build` is enough to see the change at the service's own address.

## Checks

The same as CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo llvm-cov --workspace --ignore-filename-regex 'crates/cli/' --summary-only
```

For `web/`, from that directory, after `npm ci`:

```sh
npm run lint
npm run check
npm test
npm run build
cargo build --package dalsegno
npm run e2e
```

`npm run e2e` starts `target/debug/dalsegno serve` on a temporary data directory, with no
`DALSEGNO_*` setting or config file of your own; `npx playwright install chromium` fetches
its browser once.

Run them one at a time and stop at the first failure. Never chain checks with commit, push and PR
in one command, so a failure cannot slip through to a PR.
