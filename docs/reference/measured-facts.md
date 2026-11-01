# Measured facts

These values were measured on one reference stream, are the oracle for tests, and may not hold for other streams. The reference stream loops about every 10 to 12 hours, and additions or replacements occur every few days.

## Glossary

| Term | Meaning |
| --- | --- |
| Stream | A live broadcast of one video ID. The unit registered in the tool |
| Program | The unit that groups the history of the same looping stream. Even if the stream is cut off and resumed under a new video ID, it is treated as the same program and inherits the version history |
| Short name | A short name used in file names and the like. Proposed from the stream title and confirmed by a person (example: stream title "Example TV -An Archive-" -> short name "Example TV") |
| Fragment | The smallest unit of a DASH stream. About 5 seconds for the reference stream. Video and audio are delivered as separate fragments |
| sq | The serial number of a fragment. A running count from the start of the stream; the same number for video and audio refers to the same time span |
| Stream time | The timestamp embedded in a fragment (seconds since the start of the stream). Obtained as `start_time` from ffprobe. **Does not equal sq × fragment length** ([YouTube live DASH measured facts](#youtube-live-dash-measured-facts)) |
| Wall-clock time | Japan time wall clock. Used when showing the state of a stream to people, and in file names |
| Period | The number of seconds for one pass of the playlist. The measured values for the reference stream have changed as 38,910.0 s -> 37,649.07 s -> 38,376.7 s -> 42,244.7 s |
| Loop boundary | The start of the playlist. Taking the position of the moment of switching as the reference, it repeats every period |
| Version | A playlist with a given period and a given track order. Each time a change enters, it becomes a new version |
| Addition (追加分) | A section that was not in the previous version. Besides insertions of tracks, there are also replacements that involve deleting existing parts |
| Revival | A section that is not in the immediately preceding version but was in one of the past versions. Distinguished from new |
| Known material | Material that appears many times within the loop. For the reference stream, the MV of the latest song (about 3 min 31 s, 8 to 9 times per pass) is this, and it is often placed immediately before an addition. It is not regarded as new |
| Envelope | A series obtained by converting the audio to mono 2kHz, taking the RMS every 0.1 seconds, and applying log1p (10 samples/second). All analysis in the initial implementation uses this |

## YouTube live DASH measured facts

For fragments, the reliable approach was to **obtain only the URL with yt-dlp, and then fetch `base URL + &sq=number` in parallel on our own**. yt-dlp's `--download-sections` cannot be used with `--live-from-start` ("This format cannot be partially downloaded"), and if interrupted while using `-N` the pieces are left without being concatenated, so it is not used for downloading.

### Obtaining the URL

- Obtaining: the `.url` of `yt-dlp -4 --live-from-start -f <itag> -j <video URL>` (`protocol` is `http_dash_segments_generator`)
- Expiry: `expire=<UNIX time>` in the URL is **about 6 hours after obtaining**. After expiry it returns 403. Re-obtain when less than 30 minutes remain
- IP binding: the URL contains `ip=`. **Fragments must be fetched with the same IP family (IPv4/IPv6) as when the URL was obtained**. If they differ, it becomes a hard-to-understand failure: HTTP 200 but an empty body
- Bot detection: after tens of thousands of fetches, "Sign in to confirm you're not a bot" kept appearing over IPv6 (for more than 4 days). IPv4 went through. **Prefer IPv6, and fall back to IPv4 on failure** ([IP family policy](../design/acquisition.md#ip-family-policy)). Make it possible to pass `--cookies <file>` if necessary (avoid an account used day to day)
- Cache: the obtained URL contains the IP, so if it is stored, do not let it out. Keep yt-dlp invocations to a minimum

### Response headers

Fetching any obtainable sq returns the following headers. With these, the correspondence between "now" and "the latest sq" can be fixed.

| Header | Meaning |
| --- | --- |
| `X-Head-Seqnum` | The current latest sq |
| `X-Head-Time-Sec` / `X-Head-Time-Millis` | The stream time of the latest fragment, in seconds and in milliseconds |
| `X-Walltime-Ms` | The server's **current** wall-clock time (not the time of the fetched fragment) |
| `X-Sequence-Num` | The sq of the fetched fragment |

### Handling of time

- A fragment is not strictly 5 seconds. "Stream time − sq×5" drifted by a dozen-odd seconds over a few days: about 26.6 to 27.7 seconds on 2026-10-24 and about 40.0 seconds on 2026-10-29. **Analysis must always use the measured stream time, and conversion to sq is done each time from nearby measured values**
- The DVR (range that can be rewound) is the most recent **120 hours**. It disappears from the old side

### Formats

| itag | Content | Typical size of one fragment |
| --- | --- | --- |
| 248 | VP9 1080p 30fps (webm) | about 1.1MB (about 1.8Mbps) |
| 140 | AAC 128kbps (m4a) | about 78KB |

- Each fragment has its own header, and **simply concatenating them in number order makes it playable**. When webm is concatenated, ffmpeg emits a large number of warnings "unknown-length element 0x18538067", but they are harmless
- **The start of each fragment is a keyframe**. In a file obtained from fragment boundaries, keyframes line up at 0, 5, 10... seconds from the start
- For video and audio taken over the same sq range, the difference in `start_time` is under 1 millisecond. Combining with `ffmpeg -i video -i audio -map 0:v -map 1:a -c copy` causes no audio drift

Formats (itag) and fragment lengths may differ by stream, so they are not fixed but **determined automatically when the stream is registered**. For video, the highest quality among those obtainable; for audio, AAC by default, with the option to override per stream. The table above and the numbers below are measured values for the reference stream.

### Errors and handling

| Symptom | Cause | Handling |
| --- | --- | --- |
| 403 in succession | URL expired, or IP mismatch | Re-obtain the URL. Check the IP family |
| 200 with an empty body | IP family mismatch | Treat size 0 as a failure and fetch again |
| 503 | Temporary malfunction | Wait and retry (it actually recovered) |
| Failures concentrated only near the end | Temporary limit | Wait and rerun. Lower the parallelism |
| Bot detection | Large volume of fetches in a short time | Switch to the other IP family ([IP family policy](../design/acquisition.md#ip-family-policy)), reduce the fetch volume |

The measured fetch speed is about 5 items/second at parallelism 4 (about 410KB/s for audio), and about 480 items/minute for video at parallelism 8. **It is capped by the number of requests, not by bandwidth**, so using a low-bitrate audio track hardly shortens the fetch time.

## 4.1 How to build the envelope

1. Convert the audio to mono, 2,000Hz, s16 (`ffmpeg -vn -ac 1 -ar 2000 -f s16le -`)
2. Take the RMS every 200 samples (0.1 seconds)
3. Apply `log1p` (weakens the influence of volume differences)

Decoding 48 hours of AAC and building the envelope finishes in about 3 minutes and is not a bottleneck. The envelope is computed per fragment and accumulated in the DB, and the original audio may be discarded as needed.

## 4.2 Computing the NCC

Slide a reference window r (length L) over the series e, and obtain the degree of match at each position k. The numerator is computed by FFT, and the moving average and moving variance in the denominator by cumulative sums.

```latex
\mathrm{ncc}(k) = \frac{\sum_{j} \hat{r}_j \, e_{k+j}}{L \cdot \sigma_e(k)}, \quad \hat{r} = \frac{r - \bar{r}}{\sigma_r}
```

Implementation notes (all of them were actually stepped on):

- **Mask windows with small variance**. Positions where σ_e(k) is 0.3×σ_r or less are set to 0. With silent credits and the like, the denominator approached 0 and the score exceeded 10,000
- **Compute cumulative sums in f64**. With f32, precision dropped over millions of samples and scores exceeded 1, such as 1.07
- Use different window lengths depending on purpose. 600 seconds for period estimation, 60 seconds for change tracking and version comparison. The latter can drop to 0.6 to 0.8 in degree of match, so set the threshold to 0.6 to 0.7

## 4.3 Estimating the period

Take several reference positions near the end (6 places at 30-minute intervals, 600 seconds each), and search for each one in the section a "candidate period range" earlier. The median of the lags with a degree of match of 0.9 or more is the period.

- Make the candidate range configurable. At first 9 to 12 hours was enough, but it is now 11.7 hours. For a general-purpose tool, take a wide default of 30 minutes to 24 hours, and save the period estimated for each stream to narrow the search range from the next time on
- The period obtained from audio varies by about ±0.4 seconds. The period used for cutting is refined with the dark frames of the video (5.3)

## 4.4 Tracking changes (the drift over time)

For a known period P, at each time t (2-minute steps, 60-second window), search the range P-10 minutes to P+3 hours for "how many seconds earlier the matching audio is". The result is summarized and shown as sections in which the same "increase = lag - P" continues.

- Increase stays around 0: no change
- Increase settles at a different value: that value is the net extension of the loop. New period = P + increase
- A value that jumps at a single point (such as +9,444 seconds) is because the same material, such as the recurring MV, is also at another position. Mark it as a one-off and exclude it from the judgment
- If the "moment the match breaks" is judged only from a drop in the degree of match, silent parts cause false detections. **Judge by whether the value of the lag changed**

To narrow the moment of change down to the second, display the "match with the old period" and the "match with the new period" side by side in 5-second steps. The moment the former drops is the start of the insertion, and the moment the latter finishes rising is the end (the window is 60 seconds, so there is a width of ±30 to 60 seconds).

## 4.5 Comparing versions (identifying new sections)

There is no need to catch the moment of change. **Cut out the current pass in 60-second windows at 30-second steps, and search for each in the whole of one pass of the previous version.** A window that matches nowhere is new. The reference section of the previous version straddles the loop boundary, so take one pass plus 10 minutes before and after.

The judgment is made not on individual windows but on **where the sections in which "current time - time in the previous version" is constant switch over**. Short alternations of "existing" and "new" are noise from mismatches in quiet scenes or chance matches with similar scenes. The sum of the amounts of change in the difference should agree with the increase in the period, and can be used as a check (example of 2026-10-31: 455 seconds + 3,414 seconds = 3,869 seconds, and the increase in the period is 3,868 seconds).

Notes:

- If compared only with the immediately preceding version, the revival of a song that was in an even earlier version also becomes "new" (addition A of 2026-10-31 is a case). **Keep the envelopes of all past versions, so that matching against all versions is possible**
- An insertion is sometimes a replacement that involves deleting existing parts (10/26: about 16 minutes in and about 4 minutes out for +12 minutes; 10/31: about 69 minutes in and about 12 minutes out for +57 minutes, in addition B; addition A replaced nothing)

## 4.6 Handling known material (the recurring MV and the like)

The recurring MV (about 3 min 31 s) continues to be placed immediately before the addition (on both 10/26 and 10/31). Register its envelope as known repeating material, and use it for the following.

- Exclude it from the head of the addition (move the candidate start point of the cut toward the end of the MV)
- Explain it as the cause of the "one-offs" that appear in change tracking
- When a new song comes out, the recurring MV may be replaced, so make it possible for a person to instruct registration and replacement (if detecting automatically, present "material of 3 to 4 minutes that appears several times in one pass" as candidates)

## 4.7 Improved algorithm (to be replaced in stages)

The current envelope method is sufficient for period estimation and change detection (`period` / `scan`), but **its precision is not enough to determine the breaks of new sections automatically**. Replace `diff` and `refine` with (1) to (5) below.

| Problem | What actually happened | Remedy |
| --- | --- | --- |
| The feature carries little information | Matching failed in quiet scenes, and in the 10/31 comparison "existing for only one minute" and "new" alternated finely | (2) |
| Boundary resolution is ±30 to 60 seconds | Determining the breaks relied on video and visual inspection | (3) |
| Pulled by repeating material | One-off false matches occurred with the recurring MV (8 to 9 times per pass) | (1), (4) |
| Weak against differences in volume and editing | Addition A was judged to be new (possibly a revived song) | (2), (4) |
| Thresholds are rules of thumb | 0.6, 0.7, 0.8 and 0.9 were used depending on the situation | (5) |

**(1) Section-level alignment.** For each time t of the new version, estimate the offset d = t - s from the matching position s in the previous version as **something that takes a constant value for each section**. Instead of taking the maximum match for each window, build a histogram of d for each short time span and take the d with the most votes (the same idea as offset voting in fingerprint matching). Chance matches are isolated and gather no votes, and the ends of sections become candidates for the breaks as they are. The output is a correspondence table: "section of the new version -> section of the previous version (offset d), or new".

**(2) Richer features.** Raise the decoding from 2kHz to 8 to 11kHz, and consider the following in this order.

- Band energies (log energy in 8 to 16 bands, 10 to 20 frames/second), chroma (12 dimensions), and onset strength. They can be used by simply extending the current NCC mechanism to multiple dimensions
- Landmark-style fingerprints (hashing pairs of spectral peaks). Robust against volume differences, and they give offsets in 0.1-second units and all occurrence positions of repeating material. They combine directly with the offset voting of (1)
- AAC at 128kbps (itag 140) is sufficient. So that the features can be recomputed later, `record` keeps the original audio fragments for a fixed period (example: 14 days, about 18GB)

**(3) Refining boundaries.** Only near the ends of the sections obtained in (1), compare with the position of the same offset in the previous version, using a short window (5 to 10 seconds) or a frame distance in 0.1-second units, and find the moment the similarity drops to sub-second precision. Dark frames of the video and silence are used only for the final check and for choosing the keyframe.

**(4) Excluding known material and matching against all versions.** Identify all occurrence positions of known material such as the recurring MV first, mask them, and remove them from the matching in (1). The comparison targets are all past versions, and "not in the immediately preceding version but in past version X" is displayed as a "revival", distinguished from new.

**(5) Self-verification and evaluation.** Automatically perform checks such as "the sum of the increases in offset = the increase in the period" and "length of new sections - length of deleted sections = the increase in the period", and ask a person to confirm if they do not agree. Precision is compared using the known answers of 9.1 and synthetic data (volume changes, noise, insertion of repeating material, replacement) as ground truth, in terms of the **boundary error (seconds)** and the **precision and recall of new sections**. The default thresholds are decided by this evaluation, and can be changed in the settings.

## 5.1 Detecting candidates

Use different parameters when examining the whole coarsely and when examining finely around a candidate.

| Purpose | Black (blackdetect) | Silence (silencedetect) |
| --- | --- | --- |
| Whole, coarse | `d=0.3:pix_th=0.15:pic_th=0.90` | `noise=-40dB:d=0.5` |
| ±20 seconds around a candidate | `d=0.03:pix_th=0.15:pic_th=0.90` | `noise=-40dB:d=0.1` |

- When examining around a candidate, use `ffmpeg -copyts -ss <start> -t <length>` so that results are obtained in seconds from the start of the file
- Video decoding can use `-hwaccel videotoolbox` on Apple Silicon. It takes about 7 minutes for 40 minutes, so examine the whole only when necessary
- **Candidates with only black or only silence are often in-song effects**. Prioritize positions where both overlap. However, there were also versions with no complete silence or black between songs (nothing came out at -50dB, 0.10, and it was detected after loosening to -35dB, 0.15)
- There are positions, such as between songs in live footage, where black and silence overlap but are not a "break in the program". Present multiple candidates and let a person choose by eye

## 5.2 Deciding the start point and end point

- Keyframes, for a file obtained from fragment boundaries, are every 5 seconds from the start. To check, look at the lines marked `K` with `ffprobe -read_intervals A%B -select_streams v -show_entries packet=pts_time,flags` (`-skip_frame nokey` does not work with VP9)
- **Start point**: a keyframe inside the silence/black. If there are several, a person chooses (in practice "a few seconds before the black" and "the start of the silence" were preferred)
- **End point**: even with `-c copy`, the end can be cut at any frame. Make it a position where not a single frame of the next content enters, such as the moment the sound disappears (silence_start). A piece that closes on a card in silence ends just before the sound returns instead, about 18 seconds later in the one measured case
- `-to` in `ffmpeg -ss <start> -i in -to <length> -map 0 -c copy` becomes "elapsed time from the start of the cut" when `-ss` is placed before the input. Beware of miscalculation

## 5.3 Cutting one pass and refining the period

1. Find candidates for the loop boundary from the audio (the moment the playlist switched is the start of the new version)
2. Detect fine black at both the boundary and the same position one pass later
3. The time difference of the same pair of blacks becomes the frame-accurate period (example: 166.633 -> 37,815.700 and 173.866 -> 37,822.933 are both 37,649.067 seconds. It was 0.37 seconds longer than the value from audio, 37,648.7 seconds)
4. From a keyframe slightly before the boundary, cut one pass with `-t <period>`

**A loop cut is not missing anything even if the start point is shifted**. Even cutting from 3.9 seconds before the boundary, the end just ends 3.9 seconds before the boundary, and every scene of one pass is included exactly once. However, this holds only when the same content as the previous pass continues even if the end crosses the boundary, so check that the version does not change before and after the boundary.

## 6.1 Naming

The format is `<short name> <date-time>[ addition] (<year>)`, where "addition" is the default suffix 追加分. The date-time is the wall-clock time (JST, `YYYYMMDD-HHMM`) at which that content began to flow, and the trailing `(year)` is read by Plex as the year. The short name is proposed from the title when the stream is registered and confirmed by a person, and the naming template can be changed per stream. The following are actual examples for the reference stream.

| Kind | Example | Meaning of the date-time |
| --- | --- | --- |
| One pass | `Example TV 20261024 (2026)` | The day it was obtained |
| One pass | `Example TV 20261024-2245 (2026)` | The time the playlist switched |
| Addition only | `Example TV 20261026-0249 追加分 (2026)` | The time the new part began to flow |
| Addition only | `Example TV 20261030-2356 追加分 (2026)` | The time it began to flow in the observed pass (not the time of the first broadcast) |

The folder structure is as follows.

```
<name>/
├── <name>.mp4
├── <name>.jpg            ← poster
├── <name>-fanart.jpg     ← background
└── <name>.info.json      ← for records (Plex does not read it)
```

## 6.2 How to make the mp4

- Do the cutting and the repackaging into mp4 at once with `ffmpeg -ss <start> -i <source> -to <length> -map 0 -c copy -movflags +faststart`. Putting VP9 into mp4 is not a problem
- Metadata: `title` (the name with the year removed), `artist` (the channel name), `date` (unified to the stream start date), `description` (the stream's description for one pass; for an addition, a statement that it is "only a part" and the length). For all of them, initial values are made from yt-dlp's information when the stream is registered, and a person can correct them
- Poster: for one pass, yt-dlp's thumbnail (`--write-thumbnail --convert-thumbnails jpg`); for an addition, one frame from inside it (default 60 to 120 seconds from the start, selectable in the Web UI). The background is a copy of the poster
- info.json: delete the keys that contain the IP, `formats`, `requested_formats`, `url`, `manifest_url`, `fragments`, `http_headers`, before saving

## 6.3 What actually happened with Plex

- The library is "Other Videos" (Personal Media, with Local Media Assets as top priority). Embedded metadata is read **only when first added**. Place a file only after it is complete (write under a temporary name and rename at the end)
- When a file was placed directly in the same channel folder, it was **linked to an old item whose file no longer existed** (in the DB, the new mp4 had become a part of an existing item "Example TV -An Archive- 2026-10-23 21:17"). The preventive measure is to separate folders per version. The remedy when it happens: move it out of the library -> scan -> empty the trash -> move it back and scan
- Referring to Plex's DB read-only was the reliable way to investigate (follow `media_parts` -> `media_items` -> `metadata_items` with `"/usr/lib/plexmediaserver/Plex SQLite" -readonly <library.db>`). It would be convenient to build this into the Web UI as "registration check"
- The measured storage location is the NAS (NFS, owner the library owner's uid:gid, permissions `rw-r-----`). Combining and cutting are finished in the service's work area, and only the finished product is transferred to the library (processing 100GB-class data with ffmpeg over the network is slow)
- A Plex rescan can be requested automatically after writing out, through Plex's API (`/library/sections/<id>/refresh`)

## 7.8 External dependencies

| Dependency | Purpose | Notes |
| --- | --- | --- |
| yt-dlp | Only for obtaining the URL | Updated frequently to follow YouTube-side changes. On failure, prompt `yt-dlp -U` |
| Deno | Used by yt-dlp to solve YouTube's JavaScript challenges | Effectively required. Its use can be confirmed by `[jsc:deno]` in yt-dlp's log |
| ffmpeg / ffprobe | Black/silence detection, combining, cutting, mp4 conversion, extracting the poster | Audio analysis is done with symphonia, so it is not needed for analysis alone |
| ffplay (optional) | Visual check of boundaries | Often bundled with ffmpeg |
| rsync / scp (optional) | Transfer to the NAS in `publish` | Not needed if copying directly to the mount destination |

- With `dalsegno doctor`, show the path and version of each dependency, and whether URL retrieval works for each IP family
- Confine yt-dlp invocations to one module so that it is easy to replace even if the specification changes. This is where problems are most likely to occur

## 9.1 Known answers

| Target | Value | Basis |
| --- | --- | --- |
| Period of the initial version | 38,910.0 seconds (10:48:30) | Matched to 0.1 second over 60 hours, 5 passes |
| Period of the 10/24 version | 37,649.067 seconds | Refined by black in the video (37,648.7 seconds from audio) |
| Switch to the 10/24 version | around 2026-10-24 22:45:03 | The initial version lasted until around 1 to 2 o'clock on 10/23. The roughly 45 hours in between match neither period |
| Insertion on 10/26 | starts around 02:45:45, period +727.6 seconds -> 38,376.7 seconds | The new part is about 12 minutes after the regular MV |
| Period as of 10/31 | 42,244.7 seconds (38,376.7 + 3,868.0) | Stable from 10/29 20:36 to 10/31 06:54 |
| Difference on 10/31 | Addition A +455 seconds, Addition B +3,414 seconds | Total 3,869 seconds, matching the increase in the period. Addition A is highly likely a revival of a track that was in a past version |
| Length of the regular MV | about 211 seconds | Same length immediately before the additions on 10/26 and 10/31, about 30 times in 36 hours |
| Start difference between audio and video | 0.6 milliseconds | When obtained over the same sq range |
