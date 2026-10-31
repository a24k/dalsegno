# M3 verification

Results for the exit criteria in [roadmap.md](../roadmap.md), measured on 2026-10-31 on saved
data of the reference stream. `crates/core/tests/known_answers.rs` holds these checks.

## Known answers

Both envelopes come from the same fragments. 97.8% of their values differ, and every result below
is the same to a tenth of a second.

| Known answer | Expected | Python envelope | In-process envelope |
| --- | --- | --- | --- |
| Initial period | 38,910.0 s | 38,910.1 s | not available |
| Period of the 10/24 version, from audio | 37,648.7 s | 37,648.6 s | 37,648.6 s |
| Period after the 10/26 insertion | 38,376.7 s | 38,376.6 s | 38,376.6 s |
| Size of the 10/26 insertion | +727.6 s | +727.5 to +727.7 s | +727.5 to +727.7 s |
| Period of the 10/31 version | 42,244.7 s | 42,244.7 s | 42,244.7 s |
| Addition A | +455 s | +454.3 s | +454.3 s |
| Addition B | +3,414 s | +3,413.5 s | +3,413.5 s |
| Sum, against a period increase of 3,868 s | 3,869 s | 3,867.7 s | 3,867.7 s |

This settles the question left open by milestone 2: the in-process envelope is good enough for
analysis. The fragments of the initial version are gone, so that row has only a Python result.

## Synthetic cases

All are unit tests in `crates/core/src/analysis/`.

| Case | Decision | Result |
| --- | --- | --- |
| Period shrinkage | D-01 | Found below the period |
| Long insertion | D-01 | Found by the widened search |
| Multiples of the period | D-02 | The base period is reported, also right after a change |
| Song-length net-zero replacement | D-04 | Caught by unmatched points, with no lag shift |
| Recording gaps and silence | D-05 | No data, and no false change a cycle later |

## Found on real data

Three problems showed only on the real envelopes.

- Repeated material matches its other occurrences for as long as it plays, so a shift must hold
  for 5 minutes to count. A real stretch of existing audio shorter than that is lost.
- Material about 390 s long was copied in front of addition B, and it split the addition in two
  until copies out of the old order were counted as inserted.
- A 60-second window straddling an edge matches nothing, or matches past the true edge. Edges
  are now walked at a fixed shift, to about 3 s.

## Not covered

- All of this comes from one stream. The thresholds may not suit another, and milestone 7's
  harness is what replaces them.
- Edge accuracy is measured on synthetic signals only. There is no ground truth for the real
  edges until they can be compared with the cuts in milestone 4.
- "Addition A is a revival" could not be tested: the older version's audio is not in these
  envelopes, so it is reported as new. Revival has run on synthetic data only.
- The second self-check mostly restates the first, as [analysis.md](../design/analysis.md#self-checks)
  explains.
- A replacement shorter than about 150 s between equal shifts is bridged as a quiet scene, the
  same limit as D-04. At a limit of 60 s, three new regions of 120 to 159 s appeared at unchanged
  shifts. They are taken to be quiet scenes, which nobody has confirmed by watching them.
- A hole in a stored version's cycle reads as a mismatch, not as no data. Holds keep whole cycles
  (D-11), so a stored version should have none, and nothing checks that yet.
- Known clips have run on synthetic data only, from a file. Registering them in the database
  comes with the API.
- Nothing here runs by itself yet. Stored tracking state and the change lifecycle are milestone 5.
