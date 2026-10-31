# Boundaries and export

Parameters and observed pitfalls are in
[measured-facts.md](../reference/measured-facts.md). Everything here runs through
`io::ffmpeg` with argument vectors.

## Candidates

For a region `[a, b]`:

1. Assemble the video and audio fragments covering `a ± 20 s` and `b ± 20 s` into work files.
2. Run black and silence detection with the fine parameters; if nothing is found, relax
   as measured (`-35dB`, `0.15`).
3. Score candidates: black plus silence overlap first, then either alone, marked as
   weak since those are often part of a song.
4. Attach the keyframes from `fragments` (D-06). Start candidates are the keyframes
   inside or just before an overlap, end candidates are `silence_start` positions.
5. If known material directly precedes the region, favour the start candidates at its end.
6. Generate the contact sheet and H.264 preview, at low priority.

Detection results are cached per region and shown as seek-bar marks. The tool never picks
one on its own (G3).

## Full cycle

1. Boundary candidates come from the version's anchor.
2. Detect black at the boundary and one cycle later. Equal differences across matching
   pairs give `period_video`, shown beside the audio estimate.
3. The operator selects a keyframe at or before the boundary.
4. Check that the cut lies within the version's `on_air_from_time` to `on_air_until_time`
   (FR-E5). A miss blocks export until the operator overrides it.
5. Run `ensure-range` for the cut's video and audio (FR-E13).

## Assembly

```text
sq_first = fragment whose start is the chosen keyframe
sq_last  = fragment containing start + duration
```

Fragments are concatenated byte-for-byte, per track, from `sq_first`. Video and audio then
start within a millisecond of each other, and no input `-ss` is needed (D-06).

```text
ffmpeg -i video -i audio -map 0:v -map 1:a -c copy -t <duration> \
  -movflags +faststart -metadata title=... -metadata artist=... \
  -metadata date=... -metadata description=... <work>/<name>.mp4
```

The `unknown-length element` warning from concatenated WebM is expected and filtered.
Other ffmpeg errors fail the job with the tail of stderr attached.

Verification before transfer: ffprobe confirms the duration within one frame, both
streams present, and the codecs unchanged.

## Naming

`core::naming` renders the program's template and validates the result (see
[Security](architecture.md#security)).

| Kind | Default | Datetime |
| --- | --- | --- |
| First cycle | `<name> <YYYYMMDD> (<year>)` | Date acquired |
| Cycle | `<name> <YYYYMMDD-HHMM> (<year>)` | Moment the playlist switched |
| Addition | `<name> <YYYYMMDD-HHMM> <suffix> (<year>)` | When the new part began in the observed cycle |

The suffix is program-level and defaults to 追加分.

## Folder

The container is mp4 rather than mkv because Plex does not read embedded metadata from mkv.

```text
<name>/
├── <name>.mp4
├── <name>.jpg           poster
├── <name>-fanart.jpg    copy of the poster
└── <name>.info.json     sanitised, not read by Plex
```

- Cycle poster: yt-dlp thumbnail, converted to jpg.
- Addition poster: one frame, default 60–120 s from the start, selectable in the UI.
- `info.json`: built from an allowlist, so unknown new yt-dlp keys cannot leak.

## Transfer

Plex reads embedded metadata only when first adding an item, and may link a stray file to
a stale item. So the folder must appear whole.

1. Copy to `<library>/.dalsegno-tmp/<name>/`, which Plex ignores as a dot-directory, on the
   same filesystem as the target.
2. Verify sizes, apply the configured mode.
3. Rename into `<output_dir>/<name>/`.
4. Remove local work files, once the transfer is recorded.

The job is idempotent. On retry, a target folder that passes verification counts as done,
and a leftover temporary folder is removed and copied again. A target that fails
verification is reported and never overwritten.

## Plex

Apart from the rescan request, access is read-only (D-13):

1. Request `/library/sections/<id>/refresh` with the token.
2. Poll the section for the file path, with a timeout.
3. Set `plex_status`, and show `confirmed` or `failed` with the troubleshooting hint from
   measured-facts.
