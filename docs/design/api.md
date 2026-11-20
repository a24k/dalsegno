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
| Stream detail, edit | `GET`, `PATCH /streams/{id}` | `dalsegno set --stream` |
| Start, stop | `POST /streams/{id}/start`, `/stop` | `dalsegno start`, `stop` |
| Link to program | `POST /streams/{id}/link` | `dalsegno link <id> <program>` |
| List, create programs | `GET`, `POST /programs` | `dalsegno programs`, `program` |
| Program detail, edit | `GET`, `PUT /programs/{id}` | `dalsegno program`, `set --program` |
| Versions | `GET /programs/{id}/versions` | `dalsegno versions` |
| Changes | `GET /changes`, `GET /programs/{id}/changes` | `dalsegno scan` |
| Change detail | `GET /changes/{id}` | `dalsegno diff` |
| Comparison | `GET /changes/{id}/diff` | `dalsegno diff` |
| Dismiss change | `POST /changes/{id}/dismiss` | `dalsegno dismiss` |
| Create clip | `POST /clips` | `dalsegno clip`, `cut` |
| Clip detail, edit | `GET`, `PATCH /clips/{id}` | `dalsegno clip`, `cut` |
| Boundary candidates | `GET /clips/{id}/candidates` | `dalsegno refine` |
| Preview | `GET /clips/{id}/preview` (Range) | n/a |
| Contact sheet | `GET /clips/{id}/sheet` | n/a |
| Export, transfer | `POST /clips/{id}/export` | `dalsegno cut`, `publish` |
| Known material | `GET`, `POST /programs/{id}/known-clips`, `DELETE /programs/{id}/known-clips/{clip}` | `dalsegno known` |
| Jobs | `GET`, `POST /jobs`, `POST /jobs/{id}/cancel` | `dalsegno jobs`, `fetch` |
| Settings | `GET`, `PUT /settings` | `dalsegno set` |
| Diagnostics | `GET /doctor` | `dalsegno doctor` |
| Events | `GET /events` (SSE) | n/a |

SSE events are `job.progress`, `stream.state`, `change.updated` (D-33). Each one's `data` is
the object the jobs, streams or changes list returns for that ID, sent when it is new or
differs, within about a second. A `resync` event tells a page that fell behind to refetch, and
opens each connection, so a page refetches whenever it connects. Its `data` is
`{"version": ...}`, the version of the running service, which the Web UI shows on every screen.
The Web UI reconnects with a backoff from 1 s to 30 s.

`POST /jobs` takes `{"kind": ...}` with `backfill` (`stream`, optional `hours` up to 120),
`features`, `analysis` (`stream`), `sweep`, or `fetch` (`stream`, `itag`, `first`, `last`, at
most 100000 fragments into the library), and answers `202` with the queued job. Stream IDs are
the numeric IDs of `/streams`.

`POST /streams` takes `{"stream": <video ID or URL>}`, waits for the probe, which can take
minutes, and answers `201` with the `stream` and the `probe`. A stream already registered is a
`409` `already_registered`, audio that does not pass its checks a `422` `no_audio`, and any
other failed probe a `502` `probe_failed` with the reason in the service log. `PATCH
/streams/{id}` takes `{"title"}` of 1 to 300 characters. `POST /streams/{id}/link` takes
`{"program": <id>}` and is refused with `409` when the program has versions from another
stream, which analysis cannot continue yet, or when the stream already has versions under its
own program. A program is `short_name`, `output_dir`, `name_template`, `addition_suffix`,
`artist`, `content_date` and `description`, the first two required and the template and suffix
defaulting; `POST` answers `201` and `PUT` replaces the whole of it, and each answer adds the
`stream_ids` linked to it. The folder and the template are checked before anything is stored.
`GET /programs/{id}/versions` and `/changes` page newest first. A stream stores a known clip
from its envelope with `POST /programs/{id}/known-clips`, which takes `name`, `stream`, `start`
and `end` in stream time, at most 1800 s long, and is refused with `409` where the stored
envelope does not cover the stretch. Known clips are stored now, and analysis masks them from
M7.

`GET /changes/{id}` adds the regions with their IDs, the old and new version IDs and the clips
chosen from the change. `GET /changes/{id}/diff` gives two bars, `old` and `new`, each from the
start of its cycle with its `period`, the regions in each with their IDs and the
`video_boundaries` that fall inside them, and the self-`checks`; a provisional change has no
`new` bar. `POST /changes/{id}/dismiss` closes a provisional or confirmed change, answers with
the change as `GET` does and lets its holds lapse 7 days later; a change already closed is a
`409`. A clip is a draft from its first selection until it is exported, under one ID. `POST
/clips` takes `change`, `kind` (`cycle` or `addition`), `start` and `end` in stream time and
optionally `version`, which must be the change's old or new version, and answers `201` with the
clip; a cut must start at 0 or later, end after it starts and last at most 7 days, and an
unknown change is a `404`. `PATCH /clips/{id}` takes any of `kind`, `start` and `end`, and is
refused with `409` once the clip is exported. A clip shows `id`, `state` (`draft` or
`exported`), `kind`, `change_id`, `version_id`, `stream_id`, `start`, `end`, `name`,
`duration`, `output_path` and `plex_status`.

`GET /settings` gives each operational setting with its `value`, the `default` from the file,
environment or flags, and whether it is `overridden`, and the bootstrap values with secrets and
the cookie file only as set or not. `PUT /settings` takes the keys to change, or null to drop an
override, and applies them at once (D-24); a bootstrap key is refused as `bootstrap_setting`.

`GET /doctor` runs the diagnostics in the service, and with `?stream=<id>` also resolves the
stream's URL over each family and probes its formats. A failed probe says so, with the reason in
the service log.

## CLI

`dalsegno serve` runs the service. `DALSEGNO_URL` selects it, with `http://127.0.0.1:8737` as
default. Output is human-readable by default, or `--json` for scripts.

These go through the service, and need it running:

```text
dalsegno status
dalsegno jobs
dalsegno changes
dalsegno start <id>
dalsegno stop  <id>
dalsegno add      <id>
dalsegno program  <id> --short-name <name> --output-dir <dir> [--template <t>] [--suffix <s>]
                  [--artist <a>] [--date <d>] [--description <d>]
dalsegno programs
dalsegno link     <id> <program>
dalsegno versions <program>
dalsegno known    <program> [--add <name> --stream <id> --from <s> --to <s>] [--remove <clip>]
dalsegno diff     <change>
dalsegno dismiss  <change>
dalsegno clip     --change <change> --kind <cycle|addition> --from <s> --to <s> [--version <v>]
dalsegno clip     <clip> [--kind <k>] [--from <s>] [--to <s>]
dalsegno doctor   [--stream <id>]
dalsegno set      [<key>=<value>]... [--reset <key>]...
dalsegno set      --stream <id> title=<title>
dalsegno set      --program <program> <key>=<value>...
dalsegno fetch    <id> --itag <n> --first <sq> --last <sq>
dalsegno backfill <id> [--hours <n>]
dalsegno features <id>
dalsegno sweep
```

`fetch`, `backfill`, `features` and `sweep` queue a job and print it, and `dalsegno jobs` follows
it. `doctor --stream` and `add` probe the stream, which can take a few minutes. `set` shows the
shared operational settings, and changes them when given `key=value` or `--reset key`. With
`--stream` it changes a stream's title, and with `--program` the given fields of a program,
which cannot go with `--reset`. `program` changes the program a stream has, or makes one and
links it, and `known` lists a program's known clips. `diff` shows a change's two bars, each from
the start of its cycle, with the video boundaries and the checks, and `dismiss` closes the
change. `clip` starts a draft from a change, or shows a clip, or reshapes a draft when given
`--kind`, `--from` or `--to`.

A running job that reports progress shows its share done and, from its pace so far, about how
long is left. So far only backfill reports, after each 500 fragments, and the API returns it as
`progress` from 0 to 1.

A stream shows its holes as fragments, of every format: `open_fragments` are in gaps still to be
filled, and `lost_fragments` in gaps that left the DVR first, which stay holes. `status` shows them
as `12 to fill, 0 lost`. A new or restarted stream has fragments to fill until backfill catches
up, and a lost fragment is one the recording will never have.

Offline commands read files and never need the service (FR-N11). Passing `--env` selects
offline mode; otherwise `scan` and `diff` call the API (`diff` then takes a change instead):

```text
dalsegno envelope --fragments <dir> --first <sq> --last <sq> --output <file>
dalsegno period   --env <file> [--min <s>] [--max <s>]
dalsegno scan     --env <file> --period <s> [--from <s>]
dalsegno diff     --env <file> --period <s> --against <file> --against-period <s> [--known <file>]
```

Envelope input accepts `.npy` so that existing Python results can be used directly.

Until milestone 5 these commands run in process (D-17) and need `--data-dir` or
`DALSEGNO_DATA`:

```text
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
green otherwise, as for a dismissed change. A change first seen as audio that matches nothing
has no length yet, and says so instead of a number. When `daily_summary` is set, a summary goes
out once a day at that time, or at start when the service starts later that day, with a line per stream that has not ended (state,
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
