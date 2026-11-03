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
| Changes | `GET /changes`, `GET /programs/{id}/changes` | `dalsegno scan` |
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
default. Output is human-readable by default, or `--json` for scripts.

So far `status`, `jobs`, `changes`, `start` and `stop` go through the service, and need it running:

```text
dalsegno status
dalsegno jobs
dalsegno changes
dalsegno start <id>
dalsegno stop  <id>
```

A running job that reports progress shows its share done and, from its pace so far, about how
long is left. So far only backfill reports, after each 500 fragments, and the API returns it as
`progress` from 0 to 1.

A stream shows its holes as fragments, of every format: `open_fragments` are in gaps still to be
filled, and `lost_fragments` in gaps that left the DVR first, which stay holes. `status` shows them
as `12 to fill, 0 lost`. A new or restarted stream has fragments to fill until backfill catches
up, and a lost fragment is one the recording will never have.

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

`program` is optional. A stream gets a program from its title the first time analysis or a cut
needs one, so changes are noticed without it, and `program` then changes the names in place.

Use the CLI when a judgement looks suspicious, for example a region classified as new that
may actually be existing, to inspect the internals, or to cut a manually specified range.
Each Web UI operation calls the same API, so it can be reproduced from the CLI.

## Notifications

Every notification links to the relevant Web UI screen. It opens in bold with what happened
and to which stream, then gives the details, then the link as words such as "Open the change".
Deadlines are shown in Japan time with the hours left. The `slack` format marks this up in
mrkdwn and `ntfy` gets plain text. `mattermost` sends a short message saying it is from
dalsegno and which stream it is about, and an attachment with what happened as its title
(linked to the screen), the details, and the version; its colour is red for a lost gap or a
stream error, amber for a change, a deadline, an end or a summary with losses or failures, and
green otherwise. When `daily_summary` is set, a
summary goes out once a day at that time, or at start when the service starts later that day, with a line per stream that has not ended (state,
period, fragments to fill and lost) and the jobs that failed since the last one. An
illustrative example, which is not a test oracle:

```text
dalsegno `Example TV`
| Change seen                                  (amber, linked to /changes/7)
| +455.0 s so far. Export the old version by 11/04 22:00 JST (48 hours left) before it
| leaves the DVR.
| dalsegno 0.5.0
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
