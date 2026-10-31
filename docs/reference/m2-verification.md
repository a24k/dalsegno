# M2 verification

Results for the exit criteria in [roadmap.md](../roadmap.md), measured on 2026-10-31 on the
reference stream.

| Criterion | Result |
| --- | --- |
| S4 compatibility check | Passed over 48 hours of saved fragments: correlation 0.9999999, mean absolute difference 0.00016, p99 0.00179, maximum 0.04987, and the same 1,728,050 bins as the Python envelope. |
| Incremental equals batch | An unbroken run is exact by construction, for any split into calls. Across a restart, and after a filled gap, values agree within 1e-5. |

The comparison puts bin zero at the first sample, as the Python envelope does. The product grid
is anchored to stream time, so stored bins sit up to 0.1 s from these.

These figures say the pipeline matches the existing one. Whether the envelope is good enough for
analysis is decided in milestone 3.

## Live run

`add`, `backfill`, `features` and `sweep` ran in sequence on a fresh library, with `backfill` and
`features` run twice.

- `add` chose itag 140 and itag 248.
- The second `backfill` fetched only the 5 new fragments, and the second `features` continued the
  same run, giving 20 fragments, 999 bins and one run.
- The stored `stream_time` of a fragment matched ffprobe.
- `X-Head-Time-Millis` is stream time in milliseconds, and it was recorded in `head_samples`.
- The database held no URL.

## Performance

48 hours of audio took 5 minutes 21 seconds on Apple Silicon in a release build, on one core.
That includes reading the fragments, and it is about 540 times real time.

## Not covered

- The live run was about two minutes long. Days of following are the exit of milestone 5.
- There is no follow loop yet. `head_samples` and gaps are written by `fetch` and `backfill` only.
- Holds have no owners until milestone 3 creates versions, so the sweep's handling of them has
  run in tests only.
- The sweep has only removed real fragments in tests, because nothing in the live run was old
  enough.
- Nothing creates holds yet, and video has no base retention, so `sweep` removes every video
  fragment. This follows D-12, and it is safe to run only on audio until milestone 4.
- `lag_runs` have no retention yet, as nothing writes them until milestone 3.
