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
| Sum, against a period increase of 3,868 s | 3,869 s | 3,867.8 s | 3,867.8 s |

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
- Material about 340 s long was copied in front of addition B, and it split the addition in two
  until copies out of the old order were counted as inserted.
- A 60-second window straddling an edge matches nothing, or matches past the true edge. Edges
  are now walked at a fixed shift, to about 3 s.

## Replacements

A net-zero replacement with a known answer is made by overwriting real audio with other real
audio of the same length, at about 48 places in the 10/31 cycle. The source is addition B, which
the previous version lacks. `compare` was tuned on this and on the 10/31 comparison.

| Length | Found by `compare` | Before this work |
| --- | --- | --- |
| 45 s | 31 of 48 | 0 of 48 |
| 60 s | 39 of 48 | 0 of 48 |
| 90 s | 45 of 48 | 0 of 48 |
| 120 s | 46 of 48 | 0 of 48 |
| 150 s | 47 of 47 | 3 of 47 |
| 211 s | 46 of 47 | 5 of 47 |
| 400 s | 45 of 45 | 7 of 45 |

The width limit passed its synthetic test while failing on real audio, because a window of flat
music often matches somewhere by chance in a search of a whole cycle. Bridging now asks whether
the audio at the known shift is the same, which is one comparison and not a search.

The low scores were not quiet scenes, as first assumed. These stretches are loud and flat, with
level about 9 and deviation under 0.3 against a median of 0.5. Correlation normalises by that
deviation, so it collapses, while the level stays close.

Tracking had the same weakness, and its synthetic test passed too: points that matched by chance
broke up the run of unmatched points. Everything after a lasting lag is now judged as a whole at
that lag, up to where the same lag lasts again.

| Length | Found by `scan` | Before this work |
| --- | --- | --- |
| 60 s | 24 of 48 | 0 of 48 |
| 120 s | 45 of 48 | 2 of 48 |
| 211 s | 47 of 47 | 9 of 47 |
| 400 s | 45 of 45 | 7 of 45 |

A trial counts as found when a signal covers at least half of it. No signal appeared away from a
replacement, and tracking raised no other signal. A signal runs about a window past each edge,
and neighbouring trials can join, as they sit only 700 s apart, so about 0.4 to 0.5 of the
signalled time lies outside the trials.

Tracking an unchanged cycle signals nothing on the 10/31 version, nor on the 10/24 and initial
versions, which played no part in the tuning. The initial version has only a Python envelope.

Consecutive cycles of one version, which played no part in the tuning, test for false alarms.
The 10/31 version gives one existing region and nothing new. Before this work a false new region
and a false deletion sat in the 10/31 comparison, around the repeated material after addition B.
The 10/24 version found the 10/26 insertion unprompted, when a cut was placed after it.

## Videos

Whole stretches were judged at once, so one replacement made everything around it new. Silences
between videos now cut a stretch, and each video is judged alone. These silences repeat exactly one
cycle later, sit about 4 minutes apart, and are read from the envelope (D-30). Twelve whole videos,
from one silence to the next, were overwritten with other real audio:

| Measure | Before | Now |
| --- | --- | --- |
| Videos with both edges within 30 s | 4 of 12 | 12 of 12 |
| Largest edge error | 1,443 s | 12 s |
| False new regions | 0 | 0 |

The same step gave back 45 s to 206 s of existing audio at the edges of the additions, and the
start of addition A now sits on a silence. Shift changes did not move.

Replacements placed anywhere, not at a video, gain less, because the whole video around them
stays new. For 211 s at places 2,500 s apart, audio wrongly called new fell from 3,145 s to
1,741 s, and the worst edge from 980 s to 626 s.

Judging video by video lost trials that were not at a video: 2 of 48 at 45 s, 1 at 60 s, and 1
or 2 at 120 s and 211 s with audio matched to the level. A replacement can pass inside a long video
once the rest of the stretch is given back. `scan` lost none, and the first case stays with
milestone 7.

Video by video also showed that the shifts of two regions can differ by up to the shift tolerance,
which a whole stretch hides and one video cannot bear, so a stretch may match either side.

## Compared with the Python logic

The logic of `compare.py` was run on the same trials, judged the same way.

| Length | Python | Rust |
| --- | --- | --- |
| 60 s | 29 of 48 | 39 of 48 |
| 120 s | 29 of 48 | 46 of 48 |
| 211 s | 22 of 47 | 46 of 47 |
| 400 s | 9 of 45 | 45 of 45 |

On the unchanged cycle it reports 11 false regions, 960 s in all, and Rust reports none. It was
written for a person to read, and chance matches split its regions, so a region covering half of a
trial is a hard test for it.

## Not covered

- All of this comes from one stream. The thresholds may not suit another, and milestone 7's
  harness is what replaces them.
- Edge accuracy is measured on synthetic signals only. There is no ground truth for the real
  edges until they can be compared with the cuts in milestone 4.
- "Addition A is a revival" could not be tested: the older version's audio is not in these
  envelopes, so it is reported as new. Revival has run on synthetic data only.
- The second self-check mostly restates the first, as [analysis.md](../design/analysis.md#self-checks)
  explains.
- The thresholds 0.3 and 0.35 rest on one stream, and two consecutive-cycle pairs are thin
  evidence against false alarms. Replacements of 120 s by flat audio at the same level were
  missed 18 times in 48 by `compare` and 13 by `scan`, and flatter audio would do worse.
- Digital silence lasting 2 minutes or more, in place of music, goes mostly unnoticed by `scan`
  (2 of 14 at 211 s), and `compare` calls the silence existing and only its edges new. It was
  no better before, and such content is not expected (D-05).
- Silences have been read on one stream. A stream with no silences between videos, or with
  continuous audio, keeps the behaviour of judging whole stretches. The trials replaced whole
  videos of 60 to 400 s, and which silences really are video boundaries is unknown until the cuts
  of milestone 4.
- A hole in a stored version's cycle reads as a mismatch, not as no data. Holds keep whole cycles
  (D-11), so a stored version should have none, and nothing checks that yet.
- Known clips have run on synthetic data only, from a file. Registering them in the database
  comes with the API.
- Nothing here runs by itself yet. Stored tracking state and the change lifecycle are milestone 5.
