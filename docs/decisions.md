# Decisions

Findings from reviewing the original brief, and how each is resolved. Status is
**Accepted** unless marked **Open**. Thresholds introduced here are starting
values, to be replaced by the evaluation harness (FR-N9).

## Corrections to the algorithm

### D-01 Lag tracking must also search below the period

The brief tracks lag over `P − 10 min` to `P + 3 h`. The known change from
38,910.0 s to 37,649.07 s is a drop of 1,261 s, so its lag lies outside that
range and the change would be missed.

**Decision.** Search `[max(P/2, P − 3 h), min(3P/2, P + 3 h)]`, which also keeps lag 0
and `2P` out of range for short periods. If no stable lag appears for 30 consecutive
minutes, widen to the full period range and stay widened until a run stabilises, because
a long insertion can outlast one widened search.

### D-02 Period estimation must reject harmonics

Taking the median of all lags scoring at least 0.9 mixes `P` and `2P` once the
range reaches 24 hours: a period of 11.7 hours has its harmonic at 23.4 hours.

**Decision.** For each reference position, take the smallest lag above threshold.
The median of those is the candidate. A match at `2P` adds confidence but is advisory,
because within `2P` of a change a correct period has no such match.

### D-03 History must cover the longest period

Six reference positions 30 minutes apart, each 600 s, need `P + 2.67 h` of data.
A 24-hour backfill therefore cannot confirm periods above about 21 hours, while
the search range claims 24.

**Decision.** Keep the 24-hour default. If no period is found, extend backfill
in steps to 48 hours, and to 120 hours when the operator asks, before reporting
"no loop detected". Reports state the longest period that was actually testable.

### D-04 Net-zero replacements are invisible to lag tracking

If replaced and replacement sections have equal length, the lag is identical
before and after, and "the lag value changed" never fires.

**Decision.** Track every 30 s instead of every 2 minutes, because a 2-minute step
leaves a 3:31 song with at most two wholly unmatched points. Add an *unmatched run*
signal: four or more consecutive points, non-silent in the reference window, with no
match at any lag in range. This remains robust against the silence problem because
silent reference windows are skipped rather than scored. Sections shorter than about
150 s can still go unnoticed until the harness sets a better threshold. Milestone 3 replaced
counting points with judging the stretch at the known lag, because on real audio the count
missed most replacements, and measured about 90% at 120 s.

### D-05 Silent reference windows are skipped

The brief masks silent positions in the searched series but not a silent
reference window, where normalisation divides by nearly zero.

**Decision.** A reference window with too little variance yields "no data", which
is distinct from "no match". So does a valid reference whose target at `t − P` falls in
a recording gap, otherwise every outage raises a false change one cycle later. Tracking
never treats "no data" as evidence.

### D-06 Keyframe times come from measurements

The brief says keyframes fall every 5 s from the start of a file, but also that
fragments are not exactly 5 s (stream time drifts by more than ten seconds across
days). The first rule is only an approximation.

**Decision.** Each fragment starts with a keyframe, so keyframe times are the
measured fragment start times. The cut start is selected as a `sq`, and assembly
starts at that fragment, which needs no input seek.

### D-07 Decoding must be stateful and sequential

AAC decoding reset per fragment produces a transient at each boundary, and bins
of 0.1 s do not line up with fragment boundaries. Both break "incremental equals
batch" (FR-N1). Fragments may also arrive out of order during backfill.

**Decision.** Fetching only stores fragments. Feature extraction processes contiguous
runs in `sq` order with one decoder instance per run. Each run keeps its own cursor, so
an open gap neither blocks later audio nor gets skipped for good, and all runs share one
grid anchored to stream time. The decoder resets only at a recorded gap. See
[design/analysis.md](design/analysis.md).

### D-08 Changes are provisional until a cycle closes

The brief prefetches and notifies as soon as a change is seen, yet comparing
versions needs a whole cycle, and the two additions of a single change may be
hours apart in the loop.

**Decision.** A change opens as `provisional`, using regions found so far, which
triggers prefetch and the first notification. When one full cycle has been seen
it becomes `confirmed`, the self-checks run, and a second notification reports the
result. The DVR window of 120 hours against cycles of about 12 hours makes the wait
safe. If a further change arrives first, it extends the open change.

### D-09 Unstable periods are modelled, not forced into a version

The reference stream once spent about 45 hours matching neither period.

**Decision.** A version exists only after one stable cycle. Time between versions
is a transition, held by the single open change, which notifies once.

## Gaps in the data model

### D-10 Times are scoped to a stream

`versions` and `clips` hold stream time without saying which stream, but a program
spans video IDs and every stream restarts its clock.

**Decision.** Add `stream_id` wherever a stream time or `sq` is stored.

### D-11 Features and audio for each version are held

Audio expires after 14 days, but comparing against all past versions with a future
feature set (FR-N6, milestone 7) needs the source audio.

**Decision.** Each version holds one cycle plus 10 minutes either side, because
comparison extends past the seam, of audio and features (about 0.68 GB at the reference
stream's period). Holds are exempt from retention. The envelope outside held ranges may
expire with the audio.

### D-12 Video needs a store and a retention rule

The brief defines audio retention only, while a prefetch is 1–2 GB and a full cycle
about 9 GB.

**Decision.** Replace `audio_store` with one `fragments` table and range-based
`retention_holds`, because a full cycle contains its additions, so one fragment can
have several owners. Audio has a 14-day base retention and video has none: a fragment is
removed only when past base retention and covered by no live hold. A hold outlives its
change or clip by 7 days. Video fetches are refused when free space would fall under a
configured floor, and this raises a notification.

## Scope and architecture

### D-13 Plex confirmation uses the HTTP API

The container has no access to the Plex database or `Plex SQLite` on another host.

**Decision.** Rescan and confirmation use the Plex HTTP API with a token from shared
configuration. Database inspection stays a manual troubleshooting step.

### D-14 New video IDs are registered manually at first

Nothing in the brief says how a replacement stream is discovered, and polling
channels adds yt-dlp calls.

**Decision.** The operator registers the new ID; a matching channel ID prompts a
link to the program. Automatic discovery waits for real need.

### D-15 Format selection is verified, not assumed

"Best available video" could yield AV1 or a format whose fragments do not start
with a keyframe, and the brief's guarantees were measured on VP9. On the audio side,
symphonia's AAC decoder rejects HE-AAC.

**Decision.** At registration, probe a few fragments for keyframe-first, concatenation
and mp4 compatibility. Choose the highest-quality video that passes; otherwise warn
and require a manual choice. Audio must be AAC-LC.

### D-16 Crate layout gains an adapter crate

The brief places fetching in `core` alongside pure logic.

**Decision.** `core` stays free of I/O. A new `io` crate holds yt-dlp, HTTP, ffmpeg and
Plex adapters. See [design/architecture.md](design/architecture.md).

### D-17 CLI keeps one use-case layer

Milestones 1–4 use the CLI directly and milestone 5 turns it into an API client,
which risks a rewrite.

**Decision.** Operations live as use-case functions in `app`. Early CLI commands call
them in process; at milestone 5 the API handlers wrap the same functions and CLI
dispatch switches to HTTP.

### D-18 Web UI uses Svelte

**Decision.** Svelte with Vite, built to static files and embedded with `rust-embed`.
The boundary editor needs real client state, which rules out htmx, and Svelte adds
little runtime weight.

### D-19 Public repository rules

The repository is public.

**Decision.** Public documents do not name the reference stream or channel; they say
"reference stream" and use `Example TV` in examples. Fixtures, media, cookies and the
original Japanese brief stay out of Git. Real fixtures load from a local path set by
`DALSEGNO_FIXTURES`; tests that need them are skipped without it.

### D-20 Web exposure defaults

The brief delegates authentication to a reverse proxy.

**Decision.** Keep that, and add defence in depth: bind to `127.0.0.1` unless configured,
reject state-changing requests whose `Origin` does not match, and validate video IDs and
output names strictly.

### D-21 Fragments are demuxed by our own parser

symphonia's mp4 reader expects a single `moov`, retains every `moof` in memory
(upstream issue #586) and can panic on malformed `trun` boxes (#559, #582, #583). Our
fragments each carry their own header and arrive for days.

**Decision.** A small hand-written reader extracts `tfdt`, sample sizes and the AAC
`AudioSpecificConfig` from each fragment, plus Cluster Timecode for WebM. Raw AAC packets
go straight to symphonia's AAC decoder. ffmpeg stays as a fallback for the offline path.
Spike S2 confirmed this: decoded audio matches ffmpeg at 132 dB SNR.

### D-22 Correlation and offset voting are written in-house

Prior art is a poor fit: ytarchive is a Go full-stream downloader, and is useful only as a
reference for error handling; Chromaprint has coarse offsets; `audiofp` is young. The
problem is same-audio offset matching, which is small with `realfft`.

**Decision.** Implement NCC and offset voting ourselves. Re-evaluate `audiofp` at milestone 7
only if landmark hashing is chosen.

### D-23 The DVR deadline only binds superseded versions

The brief always shows a deadline for exporting a cycle. While a version keeps looping,
though, every section comes round again, so nothing is lost by waiting.

**Decision.** While a version is current, show the rolling window as information. Once it
is superseded, the hard deadline is the **start** of its last complete cycle plus 120 hours,
since the DVR expires oldest-first, minus the estimated fetch time. It is set when the
change opens, and the `DVR deadline` notification fires from it. Anything that appeared
only once, in a transition, is deadlined from when it aired.

### D-24 Editable settings live in the database

The brief puts shared settings in a file, but also offers `PUT /settings` and a settings
screen.

**Decision.** The file and environment hold bootstrap values and secrets: paths, bind
address, cookie file, Plex token, webhook URL. Operational values (rate limits, retention,
thresholds, disk floor) start from there and can be overridden in a `settings` table. The
API rejects any attempt to change a bootstrap key.

### D-25 Notifications and analysis progress are durable

Hourly analysis and alerts must survive a crash, or the operator silently misses a change
(G2).

**Decision.** Tracking state is stored (`lag_runs`, `analysis_state`). Notifications go to an
outbox written in the same transaction as the state change, unique per kind and subject,
and a dispatcher sends them. Only one provisional change may exist per program.

### D-26 Only origin-wide failures switch IP family

One stream going private must not disturb the others.

**Decision.** The family switches on a bot-check response, or on persistent 403 or empty
responses after a URL refresh for a stream that yt-dlp still resolves. A stream that yt-dlp
reports as unavailable becomes that stream's `error`.

### D-27 Items dropped from the brief

**Decision.** `publish` writes to the mounted library, so rsync and scp are not needed. The
boundary editor replaces ffplay. Both can return if a real setup needs them.

## Notes on the brief

- §4.7 ⑤ checks "new minus deleted". Revivals and known material are inserted too, so the
  check uses inserted minus deleted (FR-N8).
- §4.5 gives "+57 min" for 10/31. That is addition B only. The net period change is
  3,868 s, about 64.5 minutes. Tests use the seconds in the known-answers table.
- The notification in §8.1 is illustrative. Its time does not match the stable range
  in §9.1, so it is not a test oracle.
- Two diagrams in §7 were lost in export. The text fully describes them, and
  [design/architecture.md](design/architecture.md) redraws them.

### D-28 Web UI and notifications are in English

**Decision.** All UI text and notifications are English, with no locale layer. Program-level
values, such as the naming suffix, are data and can be in any language.

### D-29 These documents replace the brief

**Decision.** `docs/` is the single source of truth, and the Japanese brief is retired.
Stream-specific values that must stay out of this public repository are kept with the local
fixtures. References to "the brief" in this file record where a decision came from.
