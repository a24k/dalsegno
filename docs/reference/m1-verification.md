# M1 verification

Results for the exit criteria in [roadmap.md](../roadmap.md), measured on 2026-10-31 on the
reference stream with yt-dlp 2026.10.26. These are short runs, so they show the path works, not
that it stays up for days. That is the exit of milestone 5.

| Criterion | Result |
| --- | --- |
| Same bytes as `fetch.sh` | 20 audio and 5 video fragments equal the saved ones byte for byte, and both joined files equal the same fragments joined as `fetch.sh` does. |
| Interrupted fetch resumes | A process killed with SIGKILL left 7 fragments. The rerun fetched the other 13, and the joined file was correct. |
| 403, 503, empty 200, expiry | Covered by `crates/app/tests/fetch_range.rs` against a mock server. |

No URL appeared in any report or log of these runs.

## Observed

- **IP family.** IPv6 still fails with a bot check, and IPv4 resolves. Under `v6-first` the fetch
  moved to IPv4 by itself, so the fallback has now run against the real origin. Recovering to
  IPv6 has only run against the mock.
- **Head.** A request for `sq=1` returned the head in its headers.
- **Formats.** `probe` chose the formats already in use, AAC-LC itag 140 and VP9 itag 248, and
  measured a fragment length of 4.992 s. The first candidate of each kind passed, so rejection
  has run only on damaged fixtures, not on a format the origin really offered.

## Not covered

- That kill left no temporary file. An earlier run did, with a fixed temporary name that has since
  changed. An abandoned file is ignored, which a unit test shows, and it stays on disk until the
  orphan sweep of milestone 2 removes it.
- `fetch.sh` itself was not rerun. Its saved fragments are the reference. They were fetched
  14 to 19 hours earlier, so fragment bytes were stable over at least that time.
- Fragment bytes are not parsed during a range fetch, so a 200 with a wrong body would be stored
  unless `X-Sequence-Num` disagrees. Milestone 2 reads timing from each fragment for the
  `fragments` row, which closes this.
- The disk floor and the DVR window checks of `ensure-range` belong to milestone 4.
