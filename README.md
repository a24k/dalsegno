# dalsegno

A self-hosted service that watches YouTube 24-hour streams which replay a fixed playlist (loop
streams), notices when the loop changes, and exports the new full cycle or only the added
section, losslessly, into a library folder.

The name comes from the musical D.S. (dal segno), "repeat from the sign".

> **Status:** pre-1.0. The service, its API and the CLI run in production (milestone M5); the
> Web UI is milestone M6. It is built for one operator's personal archive, not for public or
> shared instances.

## How it works

1. **Record.** Registering a video ID is all it takes. The service follows the live head one
   audio fragment at a time and fills what it missed from the DVR, so analysis always runs on
   local data. Video is fetched only for regions chosen for export or flagged as candidates.
2. **Detect.** It measures the loop period from the audio envelope and compares each cycle with
   the last. When the loop changes, it prefetches the video and sends a notification with the
   DVR deadline.
3. **Decide.** The tool proposes candidates and evidence; a person decides what is new and where
   to cut.
4. **Export.** Cuts start on keyframes and are never re-encoded. Finished folders go into the
   library, laid out so that a media server such as Plex can read them. Asking Plex to scan is
   optional.

Streams that do not loop are reported as such, not forced through the pipeline. Requests to the
origin share one rate limit, to keep its load minimal.

## Running it

The image `ghcr.io/a24k/dalsegno` holds the service, the CLI, yt-dlp and ffmpeg.

```sh
cp compose.example.yaml compose.yaml   # then set the library path and the settings below
docker compose up -d
docker compose exec dalsegno dalsegno add <video-id>
docker compose exec dalsegno dalsegno start <video-id>
docker compose exec dalsegno dalsegno status
```

The API listens on loopback only, as it has no login of its own; anything wider needs an
authenticating proxy in front (D-20). Pin an exact release or a release line such as `0.5`, so
that updates are a decision; release candidates (`-rc.N`) move neither `latest` nor a line
(D-32).

## Configuration

Settings come from `~/.config/dalsegno/config.toml` (or the file `DALSEGNO_CONFIG` names) and
from environment variables, which win. Each file key is the variable's name without the
`DALSEGNO_` prefix, in lower case. The last two rows are command options instead, read from
the environment or the command line only.

| Variable | Default | Meaning |
| --- | --- | --- |
| `DALSEGNO_DATA` | `/data` in the image | Database, fragments and work area. Required. |
| `DALSEGNO_LIBRARY` | none | Library root that finished folders move into. |
| `DALSEGNO_BIND` | `127.0.0.1:8737` | Address of the API. |
| `DALSEGNO_COOKIES` | none | Cookie file for yt-dlp. |
| `DALSEGNO_WEBHOOK` | none | Notification webhook. It can embed a secret, so it is never logged. |
| `DALSEGNO_WEBHOOK_FORMAT` | `slack` | `slack`, `mattermost` (attachments) or `ntfy` (plain text). |
| `DALSEGNO_PUBLIC_URL` | none | Base of the links in notifications. |
| `DALSEGNO_DAILY_SUMMARY` | off | Time of a daily summary, `HH:MM` in Japan time. |
| `DALSEGNO_PLEX_URL`, `DALSEGNO_PLEX_TOKEN` | none | Optional: a Plex server to ask for a scan after publishing. |
| `DALSEGNO_IP_POLICY` | `v6-first` | `v6-first`, `v4-first`, `v6-only` or `v4-only`. |
| `DALSEGNO_REQUESTS_PER_SECOND`, `DALSEGNO_CONCURRENCY` | `4`, `4` | The one request budget shared by every stream and job. |

Notifications cover a change, an error and the recovery from it, a hole that can no longer be
filled, a DVR deadline 24 hours ahead, and each start of the service. Without a webhook they
wait in the database.

## Command line

`dalsegno --help` lists every command. The main ones:

| Command | What it does |
| --- | --- |
| `serve` | Run the service. |
| `add`, `start`, `stop` | Register a stream, and start or stop following it. |
| `status`, `jobs`, `changes` | Show what the running service is doing. |
| `doctor` | Check tools, the IPv6 route and URL resolution for a stream. |
| `candidates`, `refine`, `cut`, `publish` | Find cut points, measure them, write a folder and move it into the library. |
| `envelope`, `period`, `scan`, `diff` | Analyse files offline. |

Commands that read the service go through its API (`--url`, `DALSEGNO_URL`); `--json` prints
machine-readable output.

## Documentation

- [Requirements](docs/requirements.md): goals, non-goals and what each milestone delivers.
- [Roadmap](docs/roadmap.md): milestones and their exit criteria.
- [Decisions](docs/decisions.md): where the design departs from the brief, and why.
- [Design](docs/design/): architecture, acquisition, analysis, data model, export and API.
- [Reference](docs/reference/): measured facts and the verification of each milestone.

## Development

Rust edition 2024, in a workspace whose crates depend downward only:
`core` ← `io`, `store` ← `app` ← `server`, `cli`. The checks are the same as CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo llvm-cov --workspace --ignore-filename-regex 'crates/cli/' --summary-only
```

Tests never reach YouTube. Tests that need real fixtures read them from `DALSEGNO_FIXTURES` and
are skipped without it. Contributors and agents start from [AGENTS.md](AGENTS.md).

## License

[MIT](LICENSE)
