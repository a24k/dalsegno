# M4 verification

Results for the exit criteria in [roadmap.md](../roadmap.md), measured on 2026-11-01 on saved
fragments of the reference stream, with no request to the origin. Tests that need these
fragments are in `crates/app/src/export/tests.rs` and need `DALSEGNO_FIXTURES`.

## The manual process

An addition the operator cut by hand was cut again with `dalsegno`, from the same fragments.
Packets were compared by checksum, which ignores the container.

| Track | Packets of the hand cut | Same |
| --- | --- | --- |
| Video | 123,956 | All, in order, with none extra |
| Audio | 177,944 | All but at most 1 |

`cut` also passed its own checks on this file: length within one frame, both streams present,
and codecs unchanged.

## Period from black

From black in real video at a boundary and one cycle later, `refine_period` gives 37,649.0667 s
from 11 agreeing edges. The operator measured 37,649.067 s by hand, 0.37 s longer than the period
from audio. Candidates at the same boundary rank black and silence together first, and every
keyframe offered is a stored fragment start.

## Edges against a cut

The 10/31 regions of `compare` were checked against the hand cut, the frames at each edge, and
the envelopes at a fixed shift.

| Edge | Silence there | Off by |
| --- | --- | --- |
| Start of addition A | Yes | Under 2 s |
| End of addition A | No | About 5 s, was about 165 s |
| Start of addition B | Yes | About 2 s |
| Start of the copy inside addition B | Yes | About 5 s |
| End of that copy | Yes | Under 1 s |
| End of addition B | Yes | About 0.4 s |

The end of addition A was found 165 s late, with a false deletion of the same length. Flat audio
followed it with no silence, so correlation could not place the edge and nothing split the
stretch, and the period change still added up. Edges of a change now walk into new audio in
20-second blocks at the neighbour's shift. Replacement trials are unchanged, line for line.

The cut holds this copy as well: its 394 s also still play at their old place in the loop.

## Safety

| Check | Where |
| --- | --- |
| Names cannot leave a folder, hide, or overflow, after normalising | `core::export::naming` |
| `info.json` drops every key of FR-E9 and any key it does not know | `core::export::info` |
| A folder appears whole, and a differing target is never overwritten | `io::transfer` |
| A symlink cannot lead a target out of the library | `io::transfer` |
| The Plex token is only ever in a header | `io::plex` |
| A cut is held before it is fetched, and is recorded so it can be made again | `app::export` |
| A cycle outside its air time is blocked until overridden | `app::export` |

The export tests never reach the origin: fragments are stored up front and the step that fetches
is left out. An early version of them started the real yt-dlp for an invented video ID.

## Not covered

- **Plex.** Nothing has run against a real Plex or the real library, only a mock server and a
  local folder. "Displays correctly in Plex" is the operator's check, and the exit needs it.
- **Live range fetches.** `ensure_range` was exercised only with everything already stored, and
  `candidates` and `refine` have not run against the origin.
- **A full cycle.** Cuts were additions. A cycle takes about 8,500 fragments, and the air time
  comes from flags until milestone 5 stores versions.
- **Cycle posters** are read from the stream's thumbnail through ffmpeg, which has not run.
- **Disk floor.** `ensure-range` does not yet refuse a range that would cross it.
- **Description.** `info.json` comes from the stored stream, so it holds only what `add` stored,
  and the description comes from `program` or `--description`.
- Silences between videos (D-30) were compared at these six edges only, on one stream. Five of
  the six sit on a silence, so an edge between videos can have none.
