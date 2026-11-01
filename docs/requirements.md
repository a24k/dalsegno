# dalsegno requirements

`dalsegno` is a self-hosted service that watches YouTube 24-hour streams which
replay a fixed playlist (loop streams), detects when the loop changes, and
exports the new full cycle or only the added section, losslessly, in a form
Plex can ingest.

Terms are defined in [reference/measured-facts.md](reference/measured-facts.md).
Places where these requirements deliberately differ from, or fill a gap in, the
original brief are recorded in [decisions.md](decisions.md).

The name comes from the musical D.S. (dal segno), "repeat from the sign".

## 1. Goals

| ID | Goal |
| --- | --- |
| G1 | Registering a video ID is all it takes to start monitoring. No stream-specific code or configuration. |
| G2 | The operator does nothing until notified, then finishes the job in the Web UI. |
| G3 | The tool proposes candidates and evidence. A human decides what counts as new and where to cut. |
| G4 | Exports never re-encode. |
| G5 | Load on the origin stays minimal. Personal archival use only. |
| G6 | Streams that do not loop are reported as such, not forced through the pipeline. |

Continuous recording exists because the heaviest part of the manual process was noticing a
change and then re-fetching tens of hours of audio (48 hours took 1 h 50 min), a rush against
DVR expiry that also triggered bot detection. Following the audio head one fragment at a time
costs about 17,000 requests and about 1.3 GB per day per stream, and analysis then always runs
on local data. Video is fetched from the DVR only for regions chosen for export or flagged as
candidates.

## 2. Non-goals

- Multi-user accounts, roles or tenancy. One operator, behind a trusted network.
- Automatic publishing without human boundary confirmation.
- Re-encoding, transcoding or quality conversion of exports. The only encoded
  output is short H.264 preview clips for browsers.
- Shuffled or non-looping streams, beyond reporting them.
- Downloading anything other than DASH fragments of registered streams.
- Hosting public or shared instances.

## 3. Actors and flows

**Operator** registers streams, reviews changes, picks boundaries, exports.
**Service** records, analyses, prefetches and notifies.
**Plex** consumes finished folders from the library volume.

1. *Routine (automatic).* Follow the live head one audio fragment at a time,
   analyse periodically, prefetch candidate video on change, notify.
2. *Export an addition.* Open the notification, review the version diff, adjust
   boundaries in the editor, confirm name and metadata, export, verify in Plex.
3. *Export a full cycle.* Once a new version has completed one cycle, pick the
   loop boundary and export before it leaves the DVR window.
4. *Investigate.* Use the CLI to reproduce any UI operation or to run analysis
   offline on files.

## 4. Functional requirements

Priority: **M** must, **S** should, **C** could. `MS` is the delivering milestone
from [roadmap.md](roadmap.md).

### 4.1 Stream management

| ID | Requirement | Pri | MS |
| --- | --- | --- | --- |
| FR-S1 | Register a stream from a video ID or URL; validate the ID before any external call. | M | 5 |
| FR-S2 | On registration, discover title, channel, available formats and fragment length; propose short name and metadata for confirmation. | M | 1, 5 |
| FR-S3 | Select the best available video format and AAC-LC audio automatically; allow per-stream override. | M | 1 |
| FR-S4 | Track lifecycle: `registered`, `backfilling`, `recording`, `paused`, `error`, `ended`. Show the reason for `error`. | M | 5 |
| FR-S5 | Resume every stream from its last state after a service restart. | M | 5 |
| FR-S6 | Resuming from `paused` fills the gap from the DVR. | M | 5 |
| FR-S7 | Group streams under a program so version history survives a new video ID. Suggest linking when a stream from the same channel is registered. | M | 5 |
| FR-S8 | Report "no loop detected" and suppress change detection for streams with no period. | M | 3, 5 |

### 4.2 Acquisition

| ID | Requirement | Pri | MS |
| --- | --- | --- | --- |
| FR-A1 | Resolve fragment URLs with yt-dlp only; fetch fragments directly over HTTP by `sq`. | M | 1 |
| FR-A2 | Reuse a URL per (stream, format, IP family) until 30 minutes before expiry. | M | 1 |
| FR-A3 | Keep URL resolution and fragment fetches on the same IP family as one unit. | M | 1 |
| FR-A4 | Prefer IPv6, fall back to IPv4, recover to IPv6 with growing back-off; policy configurable (v6 first, v4 first, v6 only, v4 only). | M | 1, 5 |
| FR-A5 | Treat an empty 200 as a failure. Retry 429 and 503 with exponential back-off and jitter. On repeated 403, refresh the URL, then switch family, then enter `error` and notify. | M | 1 |
| FR-A6 | Backfill oldest-first. Default 24 hours, maximum 120 hours. | M | 2 |
| FR-A7 | Write fragments atomically; interrupted work resumes without refetching. | M | 1 |
| FR-A8 | Rate and concurrency limits are global and shared; live following outranks everything; backfill and prefetch run one at a time at low priority. | M | 5 |
| FR-A9 | Read fragment timing from the fragment bytes (`tfdt`, WebM Cluster Timecode); never derive time as `sq × length`. | M | 1 |
| FR-A10 | Record head samples so `sq`, stream time and wall clock can be mapped from measurements. | M | 2 |
| FR-A11 | Record gaps explicitly. Fill them from the DVR while possible; otherwise mark them permanently lost and surface this. | M | 2, 5 |

### 4.3 Analysis

| ID | Requirement | Pri | MS |
| --- | --- | --- | --- |
| FR-N1 | Compute the 10 Hz envelope from audio; incremental results match a batch computation. | M | 2 |
| FR-N2 | Estimate period, default search range 30 minutes to 24 hours and configurable, narrowing per stream afterwards. Must not report a multiple of the true period. | M | 3 |
| FR-N3 | Track lag over time, summarised as runs; flag isolated outliers and exclude them. | M | 3 |
| FR-N4 | Detect period growth, period shrinkage and net-zero replacement. Judge by lag change or by sustained non-silent mismatch, never by a score drop alone. | M | 3 |
| FR-N5 | Compare a cycle against all previous versions, classifying regions as existing, new, revival, known material or deleted. | M | 3, 7 |
| FR-N6 | Keep features for all versions indefinitely, and enough source audio to recompute them with a future feature set. | M | 2 |
| FR-N7 | Register and replace known recurring material manually. | M | 3, 6 |
| FR-N8 | Check that shift changes sum to the period change and that inserted minus deleted equals the period change, where inserted covers new, revival and known regions; ask for human review when they disagree. | M | 3 |
| FR-N9 | Provide an evaluation harness reporting boundary error in seconds, precision/recall of new regions, and how often a confirmed boundary was among the candidates, against known answers and synthetic data. | M | 7 |
| FR-N10 | Replace `diff` and `refine` with segment alignment, richer features (decoding at 8–11 kHz; band energies, chroma, onset, then landmark fingerprints) and sub-second boundaries (5–10 s windows or 0.1 s frame distance), each step justified by the harness. Default thresholds come from the harness and stay configurable. | S | 7 |
| FR-N11 | All analysis commands run offline against files or envelopes, without the service. | M | 3 |
| FR-N12 | Suggest known-material candidates (3–4 minute segments occurring several times per cycle). | S | 7 |
| FR-N13 | Report the likely video boundaries inside each new region, as one region can join pieces of several origins. | M | 5 |

### 4.4 Boundaries and export

| ID | Requirement | Pri | MS |
| --- | --- | --- | --- |
| FR-E1 | Prefetch candidate video and audio with margins on change detection. | M | 5 |
| FR-E2 | Detect black and silence candidates, preferring overlaps; offer several and never pick silently. Offering the boundary a person wants comes before ranking it first: end candidates cover both edges of a silence, as a piece can close in silence. | M | 4 |
| FR-E3 | Refine the period from matching black intervals one cycle apart. | M | 4 |
| FR-E4 | Start points are keyframes taken from measured data; end points may be any frame. | M | 4 |
| FR-E5 | Verify that a full-cycle cut lies wholly within the period its version was on air. | M | 4 |
| FR-E6 | Produce one folder per version: `.mp4` with faststart, poster, fanart and sanitised `.info.json`. Always `-c copy`. | M | 4 |
| FR-E7 | Naming follows a per-program template; default `<name> <datetime>[ <suffix>] (<year>)` in JST. | M | 4 |
| FR-E8 | Metadata (`title`, `artist`, `date`, `description`) is proposed from stream info and editable. | M | 4 |
| FR-E9 | Strip IP-bearing keys (`formats`, `requested_formats`, `url`, `manifest_url`, `fragments`, `http_headers`) from `info.json`. | M | 4 |
| FR-E10 | Assemble locally, transfer to the library, and make the finished folder appear atomically with the configured owner and mode. | M | 4 |
| FR-E11 | Trigger a Plex rescan and confirm the item was registered. | S | 4, 6 |
| FR-E12 | Store everything needed to reproduce a cut. | M | 4 |
| FR-E13 | Fetch any fragment range an export needs but does not yet have, after checking that it is still in the DVR. | M | 4 |
| FR-E14 | Keep each boundary a person confirms, with the candidates offered for it, as known answers for FR-N9. | M | 6 |

### 4.5 Notifications and jobs

| ID | Requirement | Pri | MS |
| --- | --- | --- | --- |
| FR-J1 | Persist jobs: backfill, analysis (hourly by default), diff, prefetch, candidate extraction, export, transfer. Survive restarts. | M | 5 |
| FR-J2 | Stream job progress to the Web UI. | M | 5, 6 |
| FR-J3 | Notify on change, on error and on DVR deadline (24 hours remaining by default) via webhook (ntfy, Slack-compatible, Mattermost), linking to the relevant screen. | M | 5 |

### 4.6 Web UI

| ID | Requirement | Pri | MS |
| --- | --- | --- | --- |
| FR-U1 | Dashboard: stream states, current period, open changes with DVR time left, recent jobs. | M | 6 |
| FR-U2 | Stream management: add, start, stop, edit metadata, link to program, known material. | M | 6 |
| FR-U3 | Change detail: side-by-side version strip colour-coded by class, video boundaries inside each region, candidates, self-check results. | M | 6 |
| FR-U4 | Boundary editor: preview with black, silence and keyframe marks; keyframe snapping; numeric entry; contact sheet. A cut may start and end anywhere, not only at the edges of a region. | M | 6 |
| FR-U5 | Export: name, metadata, poster frame, run, progress, Plex confirmation. | M | 6 |
| FR-U6 | Jobs and logs; settings and diagnostics (same content as `doctor`). | M | 6 |
| FR-U7 | Preview playback supports HTTP Range and works in Safari through generated H.264 clips. | M | 6 |

### 4.7 CLI

| ID | Requirement | Pri | MS |
| --- | --- | --- | --- |
| FR-C1 | Every Web UI operation is available from the CLI through the same API. | M | 5 |
| FR-C2 | `doctor` reports dependency paths and versions, URL resolution per IP family, and the active family with the reason and time of the last switch. | M | 1, 5 |

## 5. Non-functional requirements

| ID | Category | Requirement |
| --- | --- | --- |
| NFR-1 | Origin load | About 17,000 requests per day per stream while following (audio only). yt-dlp calls kept to URL refresh. |
| NFR-2 | Reliability | Several days of continuous operation with no holes and no missed change. |
| NFR-3 | Recovery | A crash at any point loses no completed fragment and no queued job. |
| NFR-4 | Storage | Default audio retention 14 days (about 18 GB per stream), plus one held cycle per version; SQLite remains small enough to back up. Disk usage is visible and bounded by policy. |
| NFR-5 | Performance | Analysis of 120 hours of envelope completes within minutes and never delays following. |
| NFR-6 | Security | Reachable only on a LAN or VPN behind an authenticating reverse proxy. No secret or IP-bearing URL reaches logs, exports or the UI. Inputs validated at each boundary. See [design/architecture.md](design/architecture.md#security). |
| NFR-7 | Maintainability | yt-dlp usage confined to one module. Files under 800 lines, functions under 50 lines. |
| NFR-8 | Quality | At least 80% line coverage on logic crates. Unit, integration and end-to-end tests, written test-first. |
| NFR-9 | Portability | Runs in one Docker image on Linux x86-64; development and offline analysis also work on Apple Silicon. |
| NFR-10 | Repository hygiene | Public repository. No media, cookies, tokens, stream identifiers or environment details are committed. All tracked text is English, except the default naming suffix 追加分. |

## 6. Constraints

- Language: Rust, one binary `dalsegno`. `serve` is the service; other subcommands
  are API clients, except offline analysis.
- External commands: yt-dlp (URL resolution and metadata), ffmpeg and ffprobe
  (detection, assembly, cutting). Deno is required by yt-dlp.
- State lives in SQLite. Per-stream settings live in the database; only shared
  settings live in `config.toml` or the environment.
- No separate containers for yt-dlp or ffmpeg.

## 7. Release acceptance

Each milestone ends with the operator verifying on real data. Reproduction of
the known answers in [reference/measured-facts.md](reference/measured-facts.md)
is the acceptance test for analysis. The product is complete when, for a real
change, the operator goes from notification to a correct Plex item using only
the Web UI.
