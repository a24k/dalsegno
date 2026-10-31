//! Stateful AAC decoding to mono. Raw packets from the container reader go straight in (D-21).

use super::FeatureError;
use std::fmt;
use symphonia_codec_aac::AacDecoder;
use symphonia_core::codecs::audio::well_known::CODEC_ID_AAC;
use symphonia_core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia_core::packet::PacketRef;
use symphonia_core::units::{Duration, Timestamp};

const TRACK_ID: u32 = 0;

/// One decoder instance, whose state carries across packets.
pub struct AacStream {
    decoder: AacDecoder,
    sample_rate: u32,
    interleaved: Vec<f32>,
}

impl fmt::Debug for AacStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AacStream")
            .field("sample_rate", &self.sample_rate)
            .finish_non_exhaustive()
    }
}

impl AacStream {
    /// Creates a decoder from an `AudioSpecificConfig`. Only AAC-LC is accepted.
    pub fn new(audio_config: &[u8]) -> Result<Self, FeatureError> {
        let mut parameters = AudioCodecParameters::new();
        parameters.for_codec(CODEC_ID_AAC);
        parameters.with_extra_data(audio_config.into());
        let decoder = AacDecoder::try_new(&parameters, &AudioDecoderOptions::default())
            .map_err(|error| FeatureError::Unsupported(error.to_string()))?;
        let sample_rate = decoder
            .codec_params()
            .sample_rate
            .ok_or_else(|| FeatureError::Unsupported("no sample rate".to_string()))?;
        Ok(Self {
            decoder,
            sample_rate,
            interleaved: Vec::new(),
        })
    }

    /// Returns samples per second.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Decodes one packet and appends its samples to `mono`.
    pub fn decode(&mut self, packet: &[u8], mono: &mut Vec<f32>) -> Result<(), FeatureError> {
        let packet = PacketRef::new(TRACK_ID, Timestamp::ZERO, Duration::ZERO, packet);
        let decoded = self
            .decoder
            .decode_ref(&packet)
            .map_err(|error| FeatureError::Decode(error.to_string()))?;
        let channels = decoded.num_planes();
        self.interleaved.clear();
        decoded.copy_to_vec_interleaved(&mut self.interleaved);
        mono.extend(self.interleaved.chunks(channels.max(1)).map(downmix));
        Ok(())
    }
}

// Matches ffmpeg's normalised `-ac 1`, which is the mean of the channels.
fn downmix(frame: &[f32]) -> f32 {
    frame.iter().sum::<f32>() / frame.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEREO_44100_LC: [u8; 2] = [0x12, 0x10];
    const STEREO_44100_HE: [u8; 2] = [0x2A, 0x10];

    #[test]
    fn accepts_aac_lc() {
        assert_eq!(
            AacStream::new(&STEREO_44100_LC).unwrap().sample_rate(),
            44_100
        );
    }

    #[test]
    fn rejects_other_profiles() {
        assert!(matches!(
            AacStream::new(&STEREO_44100_HE),
            Err(FeatureError::Unsupported(_))
        ));
    }

    #[test]
    fn rejects_a_short_config() {
        assert!(AacStream::new(&[0x12]).is_err());
        assert!(AacStream::new(&[]).is_err());
    }

    #[test]
    fn rejects_garbage_packets_without_panicking() {
        let mut stream = AacStream::new(&STEREO_44100_LC).unwrap();
        let mut mono = Vec::new();

        for packet in [&[][..], &[0xFF; 3], &[0x00; 16], &[0xAB; 700]] {
            let _ = stream.decode(packet, &mut mono);
        }
    }

    #[test]
    fn downmix_is_the_mean() {
        assert_eq!(downmix(&[0.5, 0.25]), 0.375);
        assert_eq!(downmix(&[0.5]), 0.5);
    }
}
