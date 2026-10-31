//! Cutting audio at the silences between videos.
//!
//! Audio stops between videos, while a quiet scene keeps a noise floor, so level alone tells them
//! apart. On real audio these silences recur every cycle and sit about one song apart. They are
//! too rare to find a change with, but they say which audio belongs together once one is found.

use super::series::Prepared;

/// Envelope value below which audio has stopped.
const MAX_SILENT_LEVEL: f64 = 1.0;
/// Shorter dips happen inside videos.
const MIN_SILENT_SECONDS: f64 = 0.3;
/// A shorter stretch says too little to judge alone, so it stays with its neighbour.
const MIN_PIECE_SECONDS: f64 = 20.0;

/// Cuts `start..end` at its silences. The pieces cover the range, and meet inside a silence.
pub(super) fn split_at_silences(data: &Prepared, start: f64, end: f64) -> Vec<(f64, f64)> {
    let mut pieces = Vec::new();
    let mut from = start;
    for cut in silence_centres(data, start, end) {
        if cut - from >= MIN_PIECE_SECONDS && end - cut >= MIN_PIECE_SECONDS {
            pieces.push((from, cut));
            from = cut;
        }
    }
    pieces.push((from, end));
    pieces
}

/// Returns the silence nearest to `time`, if one is within `reach` seconds.
pub(super) fn nearest_silence(data: &Prepared, time: f64, reach: f64) -> Option<f64> {
    silence_centres(data, time - reach, time + reach)
        .into_iter()
        .min_by(|a, b| (a - time).abs().total_cmp(&(b - time).abs()))
}

fn silence_centres(data: &Prepared, start: f64, end: f64) -> Vec<f64> {
    let (Some(first), Some(last)) = (data.index_of(start), data.index_of(end)) else {
        return Vec::new();
    };
    let silent: Vec<usize> = (first..last.min(data.len()))
        .filter(|index| data.has_data(*index, 1) && data.filled[*index] < MAX_SILENT_LEVEL)
        .collect();
    silent
        .chunk_by(|a, b| b - a == 1)
        .filter(|run| run.len() >= data.samples(MIN_SILENT_SECONDS))
        .map(|run| (data.time_of(run[0]) + data.time_of(run[run.len() - 1] + 1)) / 2.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::series::Series;
    use super::super::synthetic::{content, prepared};
    use super::*;

    fn silence(seconds: f64) -> Vec<f32> {
        vec![0.0; (seconds * 10.0) as usize]
    }

    #[test]
    fn audio_is_cut_inside_each_silence() {
        let values = [content(1, 100.0), silence(2.0), content(2, 100.0)].concat();

        let pieces = split_at_silences(&prepared(&values), 0.0, 202.0);

        assert_eq!(pieces, [(0.0, 101.0), (101.0, 202.0)]);
    }

    #[test]
    fn audio_with_no_silence_stays_whole() {
        let values = content(3, 100.0);

        assert_eq!(
            split_at_silences(&prepared(&values), 10.0, 90.0),
            [(10.0, 90.0)]
        );
    }

    #[test]
    fn a_dip_shorter_than_a_silence_does_not_cut() {
        let values = [content(4, 100.0), silence(0.2), content(5, 100.0)].concat();

        assert_eq!(split_at_silences(&prepared(&values), 0.0, 200.0).len(), 1);
    }

    #[test]
    fn a_quiet_scene_does_not_cut() {
        let values = [content(6, 100.0), vec![3.2; 50], content(7, 100.0)].concat();

        assert_eq!(split_at_silences(&prepared(&values), 0.0, 205.0).len(), 1);
    }

    #[test]
    fn pieces_too_short_to_judge_stay_with_a_neighbour() {
        let values = [
            content(8, 10.0),
            silence(1.0),
            content(9, 100.0),
            silence(1.0),
            content(10, 10.0),
        ]
        .concat();

        let pieces = split_at_silences(&prepared(&values), 0.0, 122.0);

        assert_eq!(pieces, [(0.0, 122.0)]);
    }

    #[test]
    fn a_recording_gap_is_not_a_silence() {
        let values = [content(11, 100.0), silence(2.0), content(12, 100.0)].concat();
        let mut series = Series::complete(0.0, 10.0, values);
        series.valid[1_000..1_020].fill(false);

        assert_eq!(
            split_at_silences(&Prepared::new(&series), 0.0, 202.0).len(),
            1
        );
    }

    #[test]
    fn the_nearest_silence_within_reach_is_found() {
        let values = [content(14, 100.0), silence(2.0), content(15, 100.0)].concat();
        let data = prepared(&values);

        assert_eq!(nearest_silence(&data, 95.0, 10.0), Some(101.0));
        assert_eq!(nearest_silence(&data, 50.0, 10.0), None);
    }

    #[test]
    fn a_range_outside_the_series_stays_whole() {
        let data = prepared(&content(13, 10.0));

        assert_eq!(split_at_silences(&data, -5.0, 5.0), [(-5.0, 5.0)]);
    }
}
