//! Reader for the start time of a WebM fragment.

use super::ContainerError;

const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TIMECODE_SCALE: u32 = 0x2A_D7B1;
const CLUSTER: u32 = 0x1F43_B675;
const TIMECODE: u32 = 0xE7;
const DEFAULT_TIMECODE_SCALE: u64 = 1_000_000;
const NANOS_PER_SECOND: f64 = 1e9;
const MAX_ID_BYTES: usize = 4;
const MAX_SIZE_BYTES: usize = 8;

/// Returns the stream time of the first Cluster, in seconds.
pub fn parse_webm_start_time(data: &[u8]) -> Result<f64, ContainerError> {
    let mut offset = 0;
    let mut scale = DEFAULT_TIMECODE_SCALE;
    let mut in_cluster = false;
    while offset < data.len() {
        let (id, after_id) = read_id(data, offset)?;
        let (size, payload) = read_size(data, after_id)?;
        match id {
            SEGMENT | INFO => offset = payload,
            CLUSTER => {
                in_cluster = true;
                offset = payload;
            }
            TIMECODE_SCALE => {
                scale = read_uint(data, payload, known(size)?)?;
                if scale == 0 {
                    return Err(ContainerError::Unsupported("timecode scale"));
                }
                offset = payload + known(size)?;
            }
            TIMECODE if in_cluster => {
                let ticks = read_uint(data, payload, known(size)?)?;
                return Ok(ticks as f64 * scale as f64 / NANOS_PER_SECOND);
            }
            _ => offset = payload.saturating_add(known(size)?),
        }
    }
    Err(ContainerError::Missing("Cluster"))
}

fn known(size: Option<usize>) -> Result<usize, ContainerError> {
    size.ok_or(ContainerError::Unsupported("unknown-size element"))
}

fn vint_length(first: u8, max: usize) -> Result<usize, ContainerError> {
    match first.leading_zeros() as usize + 1 {
        length if length <= max => Ok(length),
        _ => Err(ContainerError::Unsupported("EBML variable-length integer")),
    }
}

fn read_id(data: &[u8], offset: usize) -> Result<(u32, usize), ContainerError> {
    let first = *data
        .get(offset)
        .ok_or(ContainerError::Truncated("EBML id"))?;
    let end = offset + vint_length(first, MAX_ID_BYTES)?;
    let bytes = data
        .get(offset..end)
        .ok_or(ContainerError::Truncated("EBML id"))?;
    let id = bytes
        .iter()
        .fold(0u32, |value, byte| (value << 8) | u32::from(*byte));
    Ok((id, end))
}

fn read_size(data: &[u8], offset: usize) -> Result<(Option<usize>, usize), ContainerError> {
    let first = *data
        .get(offset)
        .ok_or(ContainerError::Truncated("EBML size"))?;
    let length = vint_length(first, MAX_SIZE_BYTES)?;
    let end = offset + length;
    let bytes = data
        .get(offset..end)
        .ok_or(ContainerError::Truncated("EBML size"))?;
    let marker = 1u64 << (8 - length);
    let head = u64::from(first) & (marker - 1);
    let value = bytes[1..]
        .iter()
        .fold(head, |value, byte| (value << 8) | u64::from(*byte));
    let unknown = (1u64 << (7 * length)) - 1;
    let size = (value != unknown)
        .then(|| usize::try_from(value).map_err(|_| ContainerError::Truncated("EBML size")))
        .transpose()?;
    Ok((size, end))
}

fn read_uint(data: &[u8], offset: usize, size: usize) -> Result<u64, ContainerError> {
    if size > MAX_SIZE_BYTES {
        return Err(ContainerError::Unsupported("integer width"));
    }
    let bytes = data
        .get(offset..offset + size)
        .ok_or(ContainerError::Truncated("EBML integer"))?;
    Ok(bytes
        .iter()
        .fold(0, |value, byte| (value << 8) | u64::from(*byte)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEGMENT: [u8; 4] = [0x18, 0x53, 0x80, 0x67];
    const INFO: [u8; 4] = [0x15, 0x49, 0xA9, 0x66];
    const TIMECODE_SCALE: [u8; 3] = [0x2A, 0xD7, 0xB1];
    const CLUSTER: [u8; 4] = [0x1F, 0x43, 0xB6, 0x75];
    const TIMECODE: [u8; 1] = [0xE7];
    const EBML_HEADER: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];
    const UNKNOWN_SIZE: [u8; 8] = [0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

    fn element(id: &[u8], payload: &[u8]) -> Vec<u8> {
        let size = 0x0100_0000_0000_0000u64 | payload.len() as u64;
        [id, &size.to_be_bytes(), payload].concat()
    }

    fn unknown_size(id: &[u8], payload: &[u8]) -> Vec<u8> {
        [id, &UNKNOWN_SIZE, payload].concat()
    }

    fn cluster(timecode: &[u8]) -> Vec<u8> {
        unknown_size(&CLUSTER, &element(&TIMECODE, timecode))
    }

    fn file(scale: &[u8], timecode: &[u8]) -> Vec<u8> {
        let info = element(&INFO, &element(&TIMECODE_SCALE, scale));
        let segment = unknown_size(&SEGMENT, &[info, cluster(timecode)].concat());
        [element(&EBML_HEADER, &[]), segment].concat()
    }

    #[test]
    fn reads_cluster_timecode_with_default_scale() {
        let data = file(&[0x0F, 0x42, 0x40], &[0x01, 0x00, 0x00]);

        assert_eq!(parse_webm_start_time(&data), Ok(65.536));
    }

    #[test]
    fn applies_timecode_scale() {
        let data = file(&[0x07, 0xA1, 0x20], &[0x03, 0xE8]);

        assert_eq!(parse_webm_start_time(&data), Ok(0.5));
    }

    #[test]
    fn rejects_fragment_without_cluster() {
        let segment = unknown_size(&SEGMENT, &element(&INFO, &[]));

        assert_eq!(
            parse_webm_start_time(&segment),
            Err(ContainerError::Missing("Cluster"))
        );
    }

    #[test]
    fn rejects_oversized_timecode() {
        let data = file(&[0x0F, 0x42, 0x40], &[0; 9]);

        assert_eq!(
            parse_webm_start_time(&data),
            Err(ContainerError::Unsupported("integer width"))
        );
    }

    #[test]
    fn rejects_zero_timecode_scale() {
        let data = file(&[0x00], &[0x01]);

        assert_eq!(
            parse_webm_start_time(&data),
            Err(ContainerError::Unsupported("timecode scale"))
        );
    }

    #[test]
    fn ignores_timecode_outside_a_cluster() {
        let stray = element(&TIMECODE, &[0x01]);
        let segment = unknown_size(&SEGMENT, &[stray, cluster(&[0x03, 0xE8])].concat());

        assert_eq!(parse_webm_start_time(&segment), Ok(1.0));
    }

    #[test]
    fn never_panics_on_corrupted_input() {
        let data = file(&[0x0F, 0x42, 0x40], &[0x01, 0x00]);

        for index in 0..data.len() {
            for value in [0x00, 0x01, 0x7f, 0xff] {
                let corrupted: Vec<u8> = data
                    .iter()
                    .enumerate()
                    .map(|(i, byte)| if i == index { value } else { *byte })
                    .collect();
                let _ = parse_webm_start_time(&corrupted);
            }
        }
    }

    #[test]
    fn never_panics_on_truncated_input() {
        let data = file(&[0x0F, 0x42, 0x40], &[0x01, 0x00]);

        for end in 0..data.len() {
            assert!(parse_webm_start_time(&data[..end]).is_err(), "length {end}");
        }
    }
}
