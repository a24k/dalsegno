//! Choosing formats from what a stream offers (FR-S3, D-15).

use serde::Serialize;
use std::cmp::Ordering;

const AAC_LC: &str = "mp4a.40.2";

/// One format offered by a stream. Holds no URL.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FormatInfo {
    /// Format number.
    pub itag: u32,
    /// Container extension.
    pub ext: String,
    /// Video codec, absent on audio-only formats.
    pub vcodec: Option<String>,
    /// Audio codec, absent on video-only formats.
    pub acodec: Option<String>,
    /// Frame height.
    pub height: Option<u32>,
    /// Frame rate.
    pub fps: Option<f64>,
    /// Total bitrate in kbit/s.
    pub tbr: Option<f64>,
    /// Whether the format is DRM protected.
    pub has_drm: bool,
}

impl FormatInfo {
    fn is_video_only(&self) -> bool {
        self.vcodec.is_some() && self.acodec.is_none() && !self.has_drm
    }

    fn codec_rank(&self) -> Option<u8> {
        let codec = self.vcodec.as_deref()?;
        if codec.starts_with("av01") {
            Some(2)
        } else if codec.starts_with("vp9") || codec.starts_with("vp09") {
            Some(1)
        } else if codec.starts_with("avc1") {
            Some(0)
        } else {
            None
        }
    }
}

/// Returns the highest-bitrate AAC-LC format, the only audio the decoder accepts.
pub fn choose_audio(formats: &[FormatInfo]) -> Option<&FormatInfo> {
    formats
        .iter()
        .filter(|format| {
            format.acodec.as_deref() == Some(AAC_LC) && format.vcodec.is_none() && !format.has_drm
        })
        .max_by(|a, b| compare_optional(a.tbr, b.tbr))
}

/// Returns video-only formats whose codec fits an mp4, best first.
///
/// Order is height, frame rate, codec, then bitrate. Candidates still need a fragment probe.
pub fn video_candidates(formats: &[FormatInfo]) -> Vec<&FormatInfo> {
    let mut candidates: Vec<&FormatInfo> = formats
        .iter()
        .filter(|format| format.is_video_only() && format.codec_rank().is_some())
        .collect();
    candidates.sort_by(|a, b| {
        b.height
            .cmp(&a.height)
            .then_with(|| compare_optional(b.fps, a.fps))
            .then_with(|| b.codec_rank().cmp(&a.codec_rank()))
            .then_with(|| compare_optional(b.tbr, a.tbr))
    });
    candidates
}

fn compare_optional(a: Option<f64>, b: Option<f64>) -> Ordering {
    a.unwrap_or(0.0).total_cmp(&b.unwrap_or(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio(itag: u32, codec: &str, tbr: f64) -> FormatInfo {
        FormatInfo {
            itag,
            ext: "m4a".into(),
            vcodec: None,
            acodec: Some(codec.into()),
            height: None,
            fps: None,
            tbr: Some(tbr),
            has_drm: false,
        }
    }

    fn video(itag: u32, codec: &str, height: u32, fps: f64, tbr: f64) -> FormatInfo {
        FormatInfo {
            itag,
            ext: "webm".into(),
            vcodec: Some(codec.into()),
            acodec: None,
            height: Some(height),
            fps: Some(fps),
            tbr: Some(tbr),
            has_drm: false,
        }
    }

    fn itags(candidates: &[&FormatInfo]) -> Vec<u32> {
        candidates.iter().map(|format| format.itag).collect()
    }

    #[test]
    fn audio_rejects_he_aac() {
        let formats = [audio(139, "mp4a.40.5", 64.0), audio(140, AAC_LC, 144.0)];

        assert_eq!(choose_audio(&formats).unwrap().itag, 140);
    }

    #[test]
    fn audio_takes_the_highest_bitrate() {
        let formats = [audio(1, AAC_LC, 96.0), audio(2, AAC_LC, 144.0)];

        assert_eq!(choose_audio(&formats).unwrap().itag, 2);
    }

    #[test]
    fn audio_is_absent_without_aac_lc() {
        assert_eq!(choose_audio(&[audio(139, "mp4a.40.5", 64.0)]), None);
    }

    #[test]
    fn audio_skips_drm() {
        let protected = FormatInfo {
            has_drm: true,
            ..audio(140, AAC_LC, 144.0)
        };

        assert_eq!(choose_audio(&[protected]), None);
    }

    #[test]
    fn video_orders_by_height_then_frame_rate() {
        let formats = [
            video(1, "vp9", 720, 60.0, 900.0),
            video(2, "vp9", 1080, 30.0, 1800.0),
            video(3, "vp9", 1080, 60.0, 2500.0),
        ];

        assert_eq!(itags(&video_candidates(&formats)), [3, 2, 1]);
    }

    #[test]
    fn video_prefers_newer_codecs_at_equal_size() {
        let formats = [
            video(1, "avc1.640028", 1080, 30.0, 4000.0),
            video(2, "vp9", 1080, 30.0, 1800.0),
            video(3, "av01.0.08M.08", 1080, 30.0, 1500.0),
        ];

        assert_eq!(itags(&video_candidates(&formats)), [3, 2, 1]);
    }

    #[test]
    fn video_drops_unknown_codecs_muxed_and_protected_formats() {
        let muxed = FormatInfo {
            acodec: Some(AAC_LC.into()),
            ..video(1, "avc1.640028", 720, 30.0, 1000.0)
        };
        let protected = FormatInfo {
            has_drm: true,
            ..video(2, "vp9", 720, 30.0, 1000.0)
        };
        let unknown = video(3, "theora", 720, 30.0, 1000.0);

        assert!(video_candidates(&[muxed, protected, unknown]).is_empty());
    }
}
