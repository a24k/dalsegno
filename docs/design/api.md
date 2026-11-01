# API, CLI and Web UI

## Conventions

- Base path `/api/v1`. JSON.
- Envelope:

  ```json
  { "success": true, "data": {}, "error": null, "meta": { "total": 0, "page": 1, "limit": 50 } }
  ```

- Errors carry a stable `code` and a readable `message`, with internals in logs only.
- List endpoints paginate.
- Requests are parsed into validated types at the handler boundary.
- Long operations return `202` with a job ID.

## Endpoints

| Operation | Endpoint | CLI |
| --- | --- | --- |
| Register stream | `POST /streams` | `dalsegno add <id>` |
| List streams | `GET /streams` | `dalsegno status` |
| Stream detail, edit | `GET`, `PATCH /streams/{id}` | `dalsegno set` |
| Start, stop | `POST /streams/{id}/start`, `/stop` | `dalsegno start`, `stop` |
| Link to program | `POST /streams/{id}/link` | `dalsegno link <id> <program>` |
| Program detail, edit | `GET`, `PUT /programs/{id}` | `dalsegno set` |
| Versions | `GET /programs/{id}/versions` | `dalsegno versions` |
| Changes | `GET /programs/{id}/changes` | `dalsegno scan` |
| Change detail | `GET /changes/{id}` | `dalsegno diff` |
| Comparison | `GET /changes/{id}/diff` | `dalsegno diff` |
| Dismiss change | `POST /changes/{id}/dismiss` | `dalsegno dismiss` |
| Create clip | `POST /clips` | `dalsegno cut` |
| Clip detail, edit | `GET`, `PATCH /clips/{id}` | `dalsegno cut` |
| Boundary candidates | `GET /clips/{id}/candidates` | `dalsegno refine` |
| Preview | `GET /clips/{id}/preview` (Range) | n/a |
| Contact sheet | `GET /clips/{id}/sheet` | n/a |
| Export, transfer | `POST /clips/{id}/export` | `dalsegno cut`, `publish` |
| Known material | `GET`, `POST`, `DELETE /programs/{id}/known-clips` | `dalsegno known` |
| Jobs | `GET`, `POST /jobs`, `POST /jobs/{id}/cancel` | `dalsegno jobs`, `fetch` |
| Settings | `GET`, `PUT /settings` | `dalsegno set` |
| Diagnostics | `GET /doctor` | `dalsegno doctor` |
| Events | `GET /events` (SSE) | n/a |

SSE events are `job.progress`, `stream.state`, `change.updated`.

## CLI

`dalsegno serve` runs the service. `DALSEGNO_URL` selects it, with `http://127.0.0.1:8737` as
default. Output is
human-readable by default, or `--json` for scripts.

Offline commands read files and never need the service (FR-N11). Passing `--env` selects
offline mode; otherwise `scan` and `diff` call the API:

```text
dalsegno envelope --fragments <dir> --first <sq> --last <sq> --output <file>
dalsegno period   --env <file> [--min <s>] [--max <s>]
dalsegno scan     --env <file> --period <s> [--from <s>]
dalsegno diff     --env <file> --period <s> --against <file> --against-period <s> [--known <file>]
```

Envelope input accepts `.npy` so that existing Python results can be used directly.

Until milestone 5 these commands run in process (D-17). All but `doctor` and `probe` need
`--data-dir` or `DALSEGNO_DATA`:

```text
dalsegno doctor   [--stream <id>]
dalsegno probe    <id>
dalsegno add      <id>
dalsegno start    <id>
dalsegno stop     <id>
dalsegno fetch    <id> --itag <n> --first <sq> --last <sq> [--output <file>]
dalsegno backfill <id> [--hours <n>]
dalsegno features <id>
dalsegno sweep
dalsegno program    <id> --short-name <name> --output-dir <dir>
dalsegno candidates <id> --time <s>
dalsegno refine     <id> --boundary <s> --period <s>
dalsegno cut        <id> --kind <kind> --start-sq <sq> --duration <s> --named-at <unix>
dalsegno publish    <clip> --library <dir>
```

Use the CLI when a judgement looks suspicious, for example a region classified as new that
may actually be existing, to inspect the internals, or to cut a manually specified range.
Each Web UI operation calls the same API, so it can be reproduced from the CLI.

## Notifications

Every notification links to the relevant Web UI screen. An illustrative example, which is not
a test oracle:

```text
Example TV: period changed around 10/31 01:06 (38,376.7 s -> 42,244.7 s).
3 new regions (about 8, 56 and 7 min). Video already fetched.
Export the full cycle by 11/04 22:00.
```

## Web UI

Svelte SPA served from the binary. The screens walk the operator through the steps that were
previously typed as commands in a fixed order. Screens, routes and the requirements they
satisfy:

| Screen | Route | Req |
| --- | --- | --- |
| Dashboard | `/` | FR-U1 |
| Streams | `/streams`, `/streams/:id` | FR-U2 |
| Change detail | `/changes/:id` | FR-U3 |
| Boundary editor | `/clips/:id/edit` | FR-U4 |
| Export | `/clips/:id/export` | FR-U5 |
| Jobs | `/jobs` | FR-U6 |
| Settings and diagnostics | `/settings` | FR-U6 |

Notification links use these routes, built from a configured public base URL.

- **Version strip.** Two aligned bars on the time axis, colour-coded by region class, with
  text labels so that colour is not the only signal. Selecting regions creates clips.
- **Boundary editor.** Video preview, seek bar with black, silence and keyframe marks.
  Start snaps to keyframes; numeric entry is available; full keyboard operation.
- **DVR clock.** Shows remaining time wherever export is possible (D-23).
- **Preview.** H.264 clips of the boundary neighbourhood, so Safari can play them.

UI text and notifications are English (D-28).
