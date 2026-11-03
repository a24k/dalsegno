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

## Checks

The same as CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo llvm-cov --workspace --ignore-filename-regex 'crates/cli/' --summary-only
```

Run them one at a time and stop at the first failure. Never chain checks with commit, push and PR
in one command, so a failure cannot slip through to a PR.
