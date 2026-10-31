//! Reader for self-contained fragmented MP4 audio segments.

use super::ContainerError;

/// One audio fragment, borrowing from the buffer it was parsed from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mp4Fragment<'a> {
    /// Decode time of the first sample, in `timescale` ticks.
    pub start_ticks: u64,
    /// Total sample duration, in `timescale` ticks.
    pub duration_ticks: u64,
    /// Ticks per second of the track.
    pub timescale: u32,
    /// Encoded samples, in decode order.
    pub packets: Vec<&'a [u8]>,
    /// The AAC `AudioSpecificConfig`.
    pub audio_config: &'a [u8],
}

impl Mp4Fragment<'_> {
    /// Stream time of the first sample, in seconds.
    pub fn start_time(&self) -> f64 {
        self.start_ticks as f64 / f64::from(self.timescale)
    }

    /// Duration of the fragment, in seconds.
    pub fn duration(&self) -> f64 {
        self.duration_ticks as f64 / f64::from(self.timescale)
    }
}

/// Parses a fragment that carries its own `moov`, followed by one `moof` and its `mdat`.
pub fn parse_mp4_fragment(data: &[u8]) -> Result<Mp4Fragment<'_>, ContainerError> {
    let layout = top_level_boxes(data)?;
    let track = parse_moov(layout.moov)?;
    let run = parse_moof(layout.moof, track.default_duration)?;
    let end_ticks = run.start_ticks.checked_add(run.duration_ticks);
    end_ticks.ok_or(ContainerError::Unsupported("decode time"))?;
    let first = layout.first_packet_offset(run.data_offset)?;
    let packets = slice_packets(layout.mdat, first, &run.sizes)?;
    Ok(Mp4Fragment {
        start_ticks: run.start_ticks,
        duration_ticks: run.duration_ticks,
        timescale: track.timescale,
        packets,
        audio_config: track.audio_config,
    })
}

const MAX_SAMPLES: usize = 65_536;
const TFHD_BASE_DATA_OFFSET: u32 = 0x01;
const TFHD_SAMPLE_DESCRIPTION: u32 = 0x02;
const TFHD_DEFAULT_DURATION: u32 = 0x08;
const TFHD_DEFAULT_SIZE: u32 = 0x10;
const TFHD_DEFAULT_FLAGS: u32 = 0x20;
const TRUN_DATA_OFFSET: u32 = 0x01;
const TRUN_FIRST_SAMPLE_FLAGS: u32 = 0x04;
const TRUN_DURATION: u32 = 0x100;
const TRUN_SIZE: u32 = 0x200;
const TRUN_FLAGS: u32 = 0x400;
const TRUN_COMPOSITION_OFFSET: u32 = 0x800;
const SAMPLE_ENTRY_FIELDS: usize = 28;
const ES_DESCRIPTOR: u8 = 0x03;
const DECODER_CONFIG_DESCRIPTOR: u8 = 0x04;
const DECODER_SPECIFIC_INFO: u8 = 0x05;
const DECODER_CONFIG_FIELDS: usize = 13;

struct Mp4Box<'a> {
    name: [u8; 4],
    payload: &'a [u8],
    start: usize,
    payload_start: usize,
}

/// Iterates over sibling boxes, yielding an error once for a malformed header.
struct Boxes<'a> {
    data: &'a [u8],
    offset: usize,
}

fn boxes(data: &[u8]) -> Boxes<'_> {
    Boxes { data, offset: 0 }
}

impl<'a> Boxes<'a> {
    fn read(&self) -> Result<(Mp4Box<'a>, usize), ContainerError> {
        let start = self.offset;
        let short_size = be_u32(self.data, start, "box")?;
        let name = self
            .data
            .get(start + 4..start + 8)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(ContainerError::Truncated("box"))?;
        let (header, size) = match short_size {
            0 => (8, self.data.len() - start),
            1 => {
                let long = be_u64(self.data, start + 8, "box")?;
                (
                    16,
                    usize::try_from(long).map_err(|_| ContainerError::Truncated("box"))?,
                )
            }
            short => (8, short as usize),
        };
        let end = start
            .checked_add(size)
            .filter(|end| size >= header && *end <= self.data.len());
        let end = end.ok_or(ContainerError::Truncated("box"))?;
        let payload = &self.data[start + header..end];
        Ok((
            Mp4Box {
                name,
                payload,
                start,
                payload_start: start + header,
            },
            end,
        ))
    }
}

impl<'a> Iterator for Boxes<'a> {
    type Item = Result<Mp4Box<'a>, ContainerError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.data.len() {
            return None;
        }
        match self.read() {
            Ok((found, end)) => {
                self.offset = end;
                Some(Ok(found))
            }
            Err(error) => {
                self.offset = self.data.len();
                Some(Err(error))
            }
        }
    }
}

fn find<'a>(data: &'a [u8], name: &[u8; 4]) -> Result<Option<&'a [u8]>, ContainerError> {
    for found in boxes(data) {
        let found = found?;
        if &found.name == name {
            return Ok(Some(found.payload));
        }
    }
    Ok(None)
}

fn require<'a>(
    data: &'a [u8],
    name: &[u8; 4],
    label: &'static str,
) -> Result<&'a [u8], ContainerError> {
    find(data, name)?.ok_or(ContainerError::Missing(label))
}

fn be_u32(data: &[u8], offset: usize, label: &'static str) -> Result<u32, ContainerError> {
    offset
        .checked_add(4)
        .and_then(|end| data.get(offset..end))
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_be_bytes)
        .ok_or(ContainerError::Truncated(label))
}

fn be_u64(data: &[u8], offset: usize, label: &'static str) -> Result<u64, ContainerError> {
    offset
        .checked_add(8)
        .and_then(|end| data.get(offset..end))
        .and_then(|bytes| bytes.try_into().ok())
        .map(u64::from_be_bytes)
        .ok_or(ContainerError::Truncated(label))
}

/// The top-level boxes of a fragment, with what is needed to locate its samples.
struct Layout<'a> {
    moov: &'a [u8],
    moof: &'a [u8],
    moof_start: usize,
    mdat: &'a [u8],
    mdat_start: usize,
}

impl Layout<'_> {
    /// Converts a `trun` data offset, relative to the `moof`, into one relative to the `mdat`.
    fn first_packet_offset(&self, data_offset: i32) -> Result<usize, ContainerError> {
        let outside = |_| ContainerError::Truncated("mdat");
        let moof_start = i64::try_from(self.moof_start).map_err(outside)?;
        let mdat_start = i64::try_from(self.mdat_start).map_err(outside)?;
        usize::try_from(moof_start + i64::from(data_offset) - mdat_start).map_err(outside)
    }
}

fn top_level_boxes(data: &[u8]) -> Result<Layout<'_>, ContainerError> {
    let mut moov = None;
    let mut moof = None;
    for found in boxes(data) {
        let found = found?;
        match (&found.name, moov, moof) {
            (b"moov", _, _) => moov = Some(found.payload),
            (b"moof", Some(_), None) => moof = Some((found.payload, found.start)),
            (b"moof", None, _) => return Err(ContainerError::Missing("moov")),
            (b"mdat", Some(moov), Some((moof, moof_start))) => {
                return Ok(Layout {
                    moov,
                    moof,
                    moof_start,
                    mdat: found.payload,
                    mdat_start: found.payload_start,
                });
            }
            _ => {}
        }
    }
    Err(ContainerError::Missing(match (moov, moof) {
        (None, _) => "moov",
        (_, None) => "moof",
        _ => "mdat",
    }))
}

struct Track<'a> {
    timescale: u32,
    default_duration: Option<u32>,
    audio_config: &'a [u8],
}

fn parse_moov(moov: &[u8]) -> Result<Track<'_>, ContainerError> {
    let trak = require(moov, b"trak", "trak")?;
    let mdia = require(trak, b"mdia", "mdia")?;
    let timescale = parse_mdhd(require(mdia, b"mdhd", "mdhd")?)?;
    let minf = require(mdia, b"minf", "minf")?;
    let stbl = require(minf, b"stbl", "stbl")?;
    let audio_config = parse_stsd(require(stbl, b"stsd", "stsd")?)?;
    let default_duration = find(moov, b"mvex")?
        .map(|mvex| find(mvex, b"trex"))
        .transpose()?
        .flatten()
        .map(|trex| be_u32(trex, 12, "trex"))
        .transpose()?
        .filter(|duration| *duration > 0);
    Ok(Track {
        timescale,
        default_duration,
        audio_config,
    })
}

fn parse_mdhd(mdhd: &[u8]) -> Result<u32, ContainerError> {
    let offset = match mdhd.first() {
        Some(0) => 12,
        Some(1) => 20,
        Some(_) => return Err(ContainerError::Unsupported("mdhd version")),
        None => return Err(ContainerError::Truncated("mdhd")),
    };
    match be_u32(mdhd, offset, "mdhd")? {
        0 => Err(ContainerError::Unsupported("timescale")),
        timescale => Ok(timescale),
    }
}

fn parse_stsd(stsd: &[u8]) -> Result<&[u8], ContainerError> {
    let entries = stsd.get(8..).ok_or(ContainerError::Truncated("stsd"))?;
    let entry = boxes(entries)
        .next()
        .ok_or(ContainerError::Missing("sample entry"))??;
    if &entry.name != b"mp4a" {
        return Err(ContainerError::Unsupported("sample entry"));
    }
    let children = entry
        .payload
        .get(SAMPLE_ENTRY_FIELDS..)
        .ok_or(ContainerError::Truncated("mp4a"))?;
    let esds = require(children, b"esds", "esds")?;
    parse_esds(esds.get(4..).ok_or(ContainerError::Truncated("esds"))?)
}

/// Reads one descriptor, returning its tag, payload and the bytes after it.
fn read_descriptor(data: &[u8]) -> Result<(u8, &[u8], &[u8]), ContainerError> {
    const MAX_LENGTH_BYTES: usize = 4;
    let (&tag, rest) = data
        .split_first()
        .ok_or(ContainerError::Truncated("descriptor"))?;
    let length_bytes = rest
        .iter()
        .take(MAX_LENGTH_BYTES)
        .position(|byte| byte & 0x80 == 0)
        .ok_or(ContainerError::Truncated("descriptor"))?
        + 1;
    let length = rest[..length_bytes].iter().fold(0usize, |value, byte| {
        (value << 7) | usize::from(byte & 0x7f)
    });
    let body = &rest[length_bytes..];
    if length > body.len() {
        return Err(ContainerError::Truncated("descriptor"));
    }
    let (payload, after) = body.split_at(length);
    Ok((tag, payload, after))
}

fn expect_descriptor<'a>(
    data: &'a [u8],
    tag: u8,
    label: &'static str,
) -> Result<&'a [u8], ContainerError> {
    match read_descriptor(data)? {
        (found, payload, _) if found == tag => Ok(payload),
        _ => Err(ContainerError::Missing(label)),
    }
}

fn parse_esds(esds: &[u8]) -> Result<&[u8], ContainerError> {
    const STREAM_DEPENDENCE: u8 = 0x80;
    const URL: u8 = 0x40;
    const OCR_STREAM: u8 = 0x20;
    let descriptor = expect_descriptor(esds, ES_DESCRIPTOR, "ES descriptor")?;
    let flags = *descriptor
        .get(2)
        .ok_or(ContainerError::Truncated("ES descriptor"))?;
    let url_length = match flags & URL {
        0 => 0,
        _ => {
            let at = 3 + if flags & STREAM_DEPENDENCE != 0 { 2 } else { 0 };
            1 + usize::from(*descriptor.get(at).ok_or(ContainerError::Truncated("url"))?)
        }
    };
    let skipped = 3
        + if flags & STREAM_DEPENDENCE != 0 { 2 } else { 0 }
        + url_length
        + if flags & OCR_STREAM != 0 { 2 } else { 0 };
    let rest = descriptor
        .get(skipped..)
        .ok_or(ContainerError::Truncated("ES descriptor"))?;
    let config = expect_descriptor(rest, DECODER_CONFIG_DESCRIPTOR, "decoder config")?;
    let specific = config
        .get(DECODER_CONFIG_FIELDS..)
        .ok_or(ContainerError::Truncated("decoder config"))?;
    match expect_descriptor(specific, DECODER_SPECIFIC_INFO, "decoder specific info")? {
        [] => Err(ContainerError::Missing("audio config")),
        audio_config => Ok(audio_config),
    }
}

struct Run {
    start_ticks: u64,
    duration_ticks: u64,
    data_offset: i32,
    sizes: Vec<u32>,
}

#[derive(Default)]
struct SampleDefaults {
    duration: Option<u32>,
    size: Option<u32>,
}

fn parse_moof(moof: &[u8], track_duration: Option<u32>) -> Result<Run, ContainerError> {
    let traf = require(moof, b"traf", "traf")?;
    let defaults = parse_tfhd(require(traf, b"tfhd", "tfhd")?, track_duration)?;
    let start_ticks = parse_tfdt(require(traf, b"tfdt", "tfdt")?)?;
    let run_count = boxes(traf)
        .filter(|found| matches!(found, Ok(b) if &b.name == b"trun"))
        .count();
    if run_count > 1 {
        return Err(ContainerError::Unsupported("multiple trun boxes"));
    }
    parse_trun(require(traf, b"trun", "trun")?, &defaults, start_ticks)
}

fn parse_tfhd(tfhd: &[u8], track_duration: Option<u32>) -> Result<SampleDefaults, ContainerError> {
    let flags = be_u32(tfhd, 0, "tfhd")?;
    if flags & TFHD_BASE_DATA_OFFSET != 0 {
        return Err(ContainerError::Unsupported("base data offset"));
    }
    let description_bytes = if flags & TFHD_SAMPLE_DESCRIPTION != 0 {
        4
    } else {
        0
    };
    let duration_at = 8 + description_bytes;
    let (duration, size_at) = if flags & TFHD_DEFAULT_DURATION != 0 {
        (Some(be_u32(tfhd, duration_at, "tfhd")?), duration_at + 4)
    } else {
        (track_duration, duration_at)
    };
    let size = (flags & TFHD_DEFAULT_SIZE != 0)
        .then(|| be_u32(tfhd, size_at, "tfhd"))
        .transpose()?;
    let flags_at = size_at + if size.is_some() { 4 } else { 0 };
    if flags & TFHD_DEFAULT_FLAGS != 0 {
        be_u32(tfhd, flags_at, "tfhd")?;
    }
    Ok(SampleDefaults { duration, size })
}

fn parse_tfdt(tfdt: &[u8]) -> Result<u64, ContainerError> {
    match tfdt.first() {
        Some(0) => be_u32(tfdt, 4, "tfdt").map(u64::from),
        Some(1) => be_u64(tfdt, 4, "tfdt"),
        Some(_) => Err(ContainerError::Unsupported("tfdt version")),
        None => Err(ContainerError::Truncated("tfdt")),
    }
}

fn parse_trun(
    trun: &[u8],
    defaults: &SampleDefaults,
    start_ticks: u64,
) -> Result<Run, ContainerError> {
    let flags = be_u32(trun, 0, "trun")?;
    let count = be_u32(trun, 4, "trun")? as usize;
    if count > MAX_SAMPLES {
        return Err(ContainerError::Unsupported("sample count"));
    }
    if flags & TRUN_DATA_OFFSET == 0 {
        return Err(ContainerError::Missing("data offset"));
    }
    let data_offset = be_u32(trun, 8, "trun")? as i32;
    let first_row = 12
        + if flags & TRUN_FIRST_SAMPLE_FLAGS != 0 {
            4
        } else {
            0
        };
    let field = |flag: u32| if flags & flag != 0 { 4 } else { 0 };
    let has_duration = flags & TRUN_DURATION != 0;
    let size_field = field(TRUN_DURATION);
    let row = size_field + field(TRUN_SIZE) + field(TRUN_FLAGS) + field(TRUN_COMPOSITION_OFFSET);

    let samples = (0..count).map(|index| {
        let at = first_row + index * row;
        let duration = if has_duration {
            Some(be_u32(trun, at, "trun")?)
        } else {
            defaults.duration
        };
        let size = if flags & TRUN_SIZE != 0 {
            Some(be_u32(trun, at + size_field, "trun")?)
        } else {
            defaults.size
        };
        Ok((
            duration.ok_or(ContainerError::Missing("sample duration"))?,
            size.ok_or(ContainerError::Missing("sample size"))?,
        ))
    });
    let samples = samples.collect::<Result<Vec<(u32, u32)>, ContainerError>>()?;
    Ok(Run {
        start_ticks,
        duration_ticks: samples
            .iter()
            .map(|(duration, _)| u64::from(*duration))
            .sum(),
        data_offset,
        sizes: samples.iter().map(|(_, size)| *size).collect(),
    })
}

fn slice_packets<'a>(
    mdat: &'a [u8],
    first: usize,
    sizes: &[u32],
) -> Result<Vec<&'a [u8]>, ContainerError> {
    sizes
        .iter()
        .scan(first, |offset, size| {
            let begin = *offset;
            *offset = begin.saturating_add(*size as usize);
            Some((begin, *offset))
        })
        .map(|(begin, end)| {
            mdat.get(begin..end)
                .ok_or(ContainerError::Truncated("mdat"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASC: [u8; 2] = [0x12, 0x10];
    const TIMESCALE: u32 = 44_100;
    const FLAG_DEFAULT_DURATION: u32 = 0x08;
    const FLAG_DEFAULT_SIZE: u32 = 0x10;
    const FLAG_BASE_IS_MOOF: u32 = 0x02_0000;
    const TRUN_DATA_OFFSET: u32 = 0x01;
    const TRUN_DURATION: u32 = 0x100;
    const TRUN_SIZE: u32 = 0x200;

    fn boxed(name: &[u8; 4], parts: &[&[u8]]) -> Vec<u8> {
        let payload = parts.concat();
        let size = u32::try_from(payload.len() + 8).unwrap();
        [&size.to_be_bytes()[..], name, &payload].concat()
    }

    fn full(name: &[u8; 4], version: u8, flags: u32, parts: &[&[u8]]) -> Vec<u8> {
        let head = ((u32::from(version) << 24) | flags).to_be_bytes();
        boxed(name, &[&[&head[..]].concat(), &parts.concat()])
    }

    fn descriptor(tag: u8, payload: &[u8]) -> Vec<u8> {
        [&[tag, u8::try_from(payload.len()).unwrap()][..], payload].concat()
    }

    fn moov(default_duration: u32) -> Vec<u8> {
        let specific = descriptor(0x05, &ASC);
        let config = descriptor(0x04, &[&[0x40, 0x15][..], &[0; 11], &specific].concat());
        let esd = descriptor(0x03, &[&[0, 1, 0][..], &config].concat());
        let esds = full(b"esds", 0, 0, &[&esd]);
        let mp4a = boxed(b"mp4a", &[&[0; 28], &esds]);
        let stsd = full(b"stsd", 0, 0, &[&1u32.to_be_bytes(), &mp4a]);
        let stbl = boxed(b"stbl", &[&stsd]);
        let minf = boxed(b"minf", &[&stbl]);
        let mdhd = full(b"mdhd", 0, 0, &[&[0; 8], &TIMESCALE.to_be_bytes(), &[0; 8]]);
        let mdia = boxed(b"mdia", &[&mdhd, &minf]);
        let trak = boxed(b"trak", &[&mdia]);
        let trex = full(
            b"trex",
            0,
            0,
            &[
                &1u32.to_be_bytes(),
                &1u32.to_be_bytes(),
                &default_duration.to_be_bytes(),
                &[0; 8],
            ],
        );
        let mvex = boxed(b"mvex", &[&trex]);
        boxed(b"moov", &[&mvex, &trak])
    }

    struct Moof {
        tfhd_flags: u32,
        tfhd_extra: Vec<u8>,
        tfdt: Vec<u8>,
        trun_flags: u32,
        rows: Vec<u8>,
        count: u32,
        rewind: u32,
    }

    fn fragment(moov: &[u8], moof: &Moof, mdat: &[u8]) -> Vec<u8> {
        let build = |offset: u32| {
            let tfhd = full(
                b"tfhd",
                0,
                moof.tfhd_flags,
                &[&1u32.to_be_bytes(), &moof.tfhd_extra],
            );
            let trun = full(
                b"trun",
                0,
                moof.trun_flags,
                &[
                    &moof.count.to_be_bytes(),
                    &offset.wrapping_sub(moof.rewind).to_be_bytes(),
                    &moof.rows,
                ],
            );
            let traf = boxed(b"traf", &[&tfhd, &moof.tfdt, &trun]);
            boxed(b"moof", &[&traf])
        };
        let offset = u32::try_from(build(0).len() + 8).unwrap();
        [moov, &build(offset), &boxed(b"mdat", &[mdat])].concat()
    }

    fn explicit_rows() -> Moof {
        let rows: Vec<u8> = [(1024u32, 2u32), (1024, 3)]
            .iter()
            .flat_map(|(duration, size)| [duration.to_be_bytes(), size.to_be_bytes()])
            .flatten()
            .collect();
        Moof {
            tfhd_flags: FLAG_BASE_IS_MOOF,
            tfhd_extra: Vec::new(),
            tfdt: full(b"tfdt", 0, 0, &[&44_100u32.to_be_bytes()]),
            trun_flags: TRUN_DATA_OFFSET | TRUN_DURATION | TRUN_SIZE,
            rows,
            count: 2,
            rewind: 0,
        }
    }

    #[test]
    fn reads_timing_packets_and_audio_config() {
        let data = fragment(&moov(0), &explicit_rows(), &[1, 2, 3, 4, 5]);

        let parsed = parse_mp4_fragment(&data).unwrap();

        assert_eq!(parsed.start_ticks, 44_100);
        assert_eq!(parsed.duration_ticks, 2_048);
        assert_eq!(parsed.timescale, TIMESCALE);
        assert_eq!(parsed.packets, vec![&[1u8, 2][..], &[3, 4, 5][..]]);
        assert_eq!(parsed.audio_config, &ASC);
        assert!((parsed.start_time() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn reads_64_bit_decode_time() {
        let start = 1u64 << 40;
        let moof = Moof {
            tfdt: full(b"tfdt", 1, 0, &[&start.to_be_bytes()]),
            ..explicit_rows()
        };
        let data = fragment(&moov(0), &moof, &[1, 2, 3, 4, 5]);

        assert_eq!(parse_mp4_fragment(&data).unwrap().start_ticks, start);
    }

    #[test]
    fn falls_back_to_track_fragment_defaults() {
        let moof = Moof {
            tfhd_flags: FLAG_BASE_IS_MOOF | FLAG_DEFAULT_DURATION | FLAG_DEFAULT_SIZE,
            tfhd_extra: [1000u32.to_be_bytes(), 2u32.to_be_bytes()].concat(),
            trun_flags: TRUN_DATA_OFFSET,
            rows: Vec::new(),
            ..explicit_rows()
        };
        let data = fragment(&moov(0), &moof, &[1, 2, 3, 4]);

        let parsed = parse_mp4_fragment(&data).unwrap();

        assert_eq!(parsed.duration_ticks, 2_000);
        assert_eq!(parsed.packets, vec![&[1u8, 2][..], &[3, 4][..]]);
    }

    #[test]
    fn falls_back_to_track_extends_duration() {
        let moof = Moof {
            tfhd_flags: FLAG_BASE_IS_MOOF | FLAG_DEFAULT_SIZE,
            tfhd_extra: 2u32.to_be_bytes().to_vec(),
            trun_flags: TRUN_DATA_OFFSET,
            rows: Vec::new(),
            ..explicit_rows()
        };
        let data = fragment(&moov(1024), &moof, &[1, 2, 3, 4]);

        assert_eq!(parse_mp4_fragment(&data).unwrap().duration_ticks, 2_048);
    }

    #[test]
    fn rejects_packets_outside_the_buffer() {
        let data = fragment(&moov(0), &explicit_rows(), &[1, 2]);

        assert_eq!(
            parse_mp4_fragment(&data),
            Err(ContainerError::Truncated("mdat"))
        );
    }

    #[test]
    fn rejects_packets_outside_mdat() {
        let moof = Moof {
            rewind: 8,
            ..explicit_rows()
        };
        let data = fragment(&moov(0), &moof, &[1, 2, 3, 4, 5]);

        assert_eq!(
            parse_mp4_fragment(&data),
            Err(ContainerError::Truncated("mdat"))
        );
    }

    #[test]
    fn rejects_end_time_that_overflows() {
        let moof = Moof {
            tfdt: full(b"tfdt", 1, 0, &[&u64::MAX.to_be_bytes()]),
            ..explicit_rows()
        };
        let data = fragment(&moov(0), &moof, &[1, 2, 3, 4, 5]);

        assert_eq!(
            parse_mp4_fragment(&data),
            Err(ContainerError::Unsupported("decode time"))
        );
    }

    #[test]
    fn rejects_fragment_without_moof() {
        assert_eq!(
            parse_mp4_fragment(&moov(0)),
            Err(ContainerError::Missing("moof"))
        );
    }

    #[test]
    fn never_panics_on_truncated_input() {
        let data = fragment(&moov(0), &explicit_rows(), &[1, 2, 3, 4, 5]);

        for end in 0..data.len() {
            assert!(parse_mp4_fragment(&data[..end]).is_err(), "length {end}");
        }
    }

    #[test]
    fn never_panics_on_corrupted_input() {
        let data = fragment(&moov(0), &explicit_rows(), &[1, 2, 3, 4, 5]);

        for index in 0..data.len() {
            for value in [0x00, 0x7f, 0xff] {
                let corrupted: Vec<u8> = data
                    .iter()
                    .enumerate()
                    .map(|(i, byte)| if i == index { value } else { *byte })
                    .collect();
                let _ = parse_mp4_fragment(&corrupted);
            }
        }
    }
}
