# M0 spikes

Results of the spikes in [roadmap.md](../roadmap.md), measured on 2026-10-31 on the reference
stream. One live sample and one hour of saved fragments, so treat them as observations and
keep the code tolerant.

## S1 Origin behaviour around the head

Seven requests over IPv4 with an `itag 140` URL.

| Request | Result | Time |
| --- | --- | --- |
| `sq` = head | 200, normal size | 0.04 s |
| `sq` = head + 1 | 200, normal size | 0.47 s, held until it existed |
| `sq` = head + 2 | 200, normal size | 4.98 s, held until it existed |
| `sq` = head + 20 | 404, empty | 7.81 s |
| `sq` about 121 hours behind head | 200, normal size | 0.46 s |

- The head fragment is complete, so there is no need to stop at head − 1.
- A request just past the head is a long poll. The follower asks for the next `sq` and needs
  no timer. Its request timeout must exceed one fragment duration by a wide margin.
- `X-Head-Seqnum` is present on the 404 too, so a follower that fell behind can resynchronise.
- The DVR kept a fragment older than 120 hours. Deadlines stay at 120 hours as the safe bound.

## S2 Decoding

Hand-written container reader plus symphonia 0.6 `AacDecoder`, fed raw packets with the
`AudioSpecificConfig` from each fragment. 720 fragments, about one hour. 158,778,368 frames,
no decode failures.

| Decoder state | SNR against ffmpeg |
| --- | --- |
| One decoder across fragments | 132.3 dB |
| Reset at every fragment | 26.6 dB |

Sample-accurate alignment with ffmpeg at lag 0. D-07 and D-21 are confirmed: continuous
decoding is equivalent to ffmpeg, and resetting per fragment is not.

ffmpeg's float output skips downmix normalisation, so its stereo-to-mono level is √2 higher
than its `s16` output. Correct for this before comparing PCM.

## S3 Timing

`tfdt` and Cluster Timecode match ffprobe `start_time` within 1 ms on all probed fragments.
Over 200 consecutive audio fragments, each start plus duration equals the next start, to the
tick. This is covered by `crates/core/tests/fixture_fragments.rs`.

Each audio fragment holds `ftyp`, `moov`, `emsg`, `moof` and `mdat`. Audio is AAC-LC, stereo,
44.1 kHz. Durations follow AAC frames, for example 5.016 s then 4.992 s, which is one reason
`sq × length` drifts.

## S4 Envelope

Against the Python envelope over 35,998 bins:

| Pipeline | Correlation | Mean abs error | p99 | Max |
| --- | --- | --- | --- | --- |
| ffmpeg with the Python recipe | 1.000000 | 0.00000 | 0.00000 | 0.00000 |
| In process, continuous decoding | 0.999999 | 0.00029 | 0.00297 | 0.11143 |
| In process, reset per fragment | 0.999980 | 0.00150 | 0.05099 | 0.32391 |

The in-process path used a 16-zero-crossing Blackman-windowed sinc at 0.97 of Nyquist. Decoded
PCM is equivalent, so the remaining difference comes from the resampler. Exact equality would
mean reimplementing ffmpeg's resampler, which brings no analysis benefit.

**Acceptance for milestone 2:** correlation of at least 0.9999, mean absolute error of at most
0.001 and p99 of at most 0.005 against the Python envelope.

The Python envelope and the first decoded sample lined up at offset 0 on this data, because the
fixture starts at a fragment boundary.
