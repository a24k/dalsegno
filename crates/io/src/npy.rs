//! Envelopes in NumPy's `.npy` format, so existing Python results can be used directly.

use std::fs;
use std::io;
use std::path::Path;

const MAGIC: &[u8] = b"\x93NUMPY";
const VERSION_1: [u8; 2] = [1, 0];
const PREAMBLE_BYTES: usize = 10;
const HEADER_ALIGNMENT: usize = 64;
const F32_BYTES: usize = size_of::<f32>();

/// Why a file could not be used.
#[derive(Debug, thiserror::Error)]
pub enum NpyError {
    /// The file could not be read or written.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// The file is not a one-dimensional little-endian `f32` array.
    #[error("not a supported npy file: {0}")]
    Format(&'static str),
}

/// Reads a one-dimensional little-endian `f32` array.
pub fn read_f32(path: &Path) -> Result<Vec<f32>, NpyError> {
    parse_f32(&fs::read(path)?)
}

fn parse_f32(bytes: &[u8]) -> Result<Vec<f32>, NpyError> {
    let preamble = bytes
        .get(..PREAMBLE_BYTES)
        .ok_or(NpyError::Format("too short"))?;
    if &preamble[..MAGIC.len()] != MAGIC {
        return Err(NpyError::Format("wrong magic"));
    }
    if preamble[6..8] != VERSION_1 {
        return Err(NpyError::Format("only version 1 is read"));
    }
    let header_len = usize::from(u16::from_le_bytes([preamble[8], preamble[9]]));
    let data_start = PREAMBLE_BYTES + header_len;
    let header = bytes
        .get(PREAMBLE_BYTES..data_start)
        .ok_or(NpyError::Format("truncated header"))?;
    let header = std::str::from_utf8(header).map_err(|_| NpyError::Format("header encoding"))?;
    if !header.contains("'<f4'") {
        return Err(NpyError::Format("expected little-endian f32"));
    }
    if !header.contains("'fortran_order': False") {
        return Err(NpyError::Format("expected C order"));
    }
    let count = shape_of(header)?;
    let (chunks, rest) = bytes[data_start..].as_chunks::<F32_BYTES>();
    if chunks.len() != count || !rest.is_empty() {
        return Err(NpyError::Format("data length differs from the shape"));
    }
    Ok(chunks
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect())
}

fn shape_of(header: &str) -> Result<usize, NpyError> {
    let after = header
        .split_once("'shape':")
        .ok_or(NpyError::Format("no shape"))?
        .1;
    let inside = after
        .split_once('(')
        .and_then(|(_, rest)| rest.split_once(')'))
        .ok_or(NpyError::Format("malformed shape"))?
        .0;
    let mut dimensions = inside
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty());
    let count = dimensions
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or(NpyError::Format("malformed shape"))?;
    if dimensions.next().is_some() {
        return Err(NpyError::Format("expected one dimension"));
    }
    Ok(count)
}

/// Writes a one-dimensional little-endian `f32` array, which appears only when complete.
pub fn write_f32(path: &Path, values: &[f32]) -> Result<(), NpyError> {
    let dictionary = format!(
        "{{'descr': '<f4', 'fortran_order': False, 'shape': ({},), }}",
        values.len()
    );
    let unpadded = PREAMBLE_BYTES + dictionary.len() + 1;
    let padding = unpadded.next_multiple_of(HEADER_ALIGNMENT) - unpadded;
    let header = format!("{dictionary}{}\n", " ".repeat(padding));
    let header_len = u16::try_from(header.len()).map_err(|_| NpyError::Format("long header"))?;
    let mut bytes = Vec::with_capacity(PREAMBLE_BYTES + header.len() + values.len() * F32_BYTES);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION_1);
    bytes.extend_from_slice(&header_len.to_le_bytes());
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend(values.iter().flat_map(|value| value.to_le_bytes()));
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".part");
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(header: &str, data: &[u8]) -> Vec<u8> {
        let length = header.len() as u16;
        [
            MAGIC,
            &VERSION_1,
            &length.to_le_bytes(),
            header.as_bytes(),
            data,
        ]
        .concat()
    }

    const GOOD: &str = "{'descr': '<f4', 'fortran_order': False, 'shape': (1,), }";

    #[test]
    fn what_is_written_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("env.npy");
        let values = [1.5, -2.25, 0.0];

        write_f32(&path, &values).unwrap();

        assert_eq!(read_f32(&path).unwrap(), values);
    }

    #[test]
    fn the_header_is_aligned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("env.npy");

        write_f32(&path, &[]).unwrap();

        let length = fs::metadata(&path).unwrap().len() as usize;
        assert_eq!(length % HEADER_ALIGNMENT, 0);
    }

    #[test]
    fn reads_the_format_python_writes() {
        let values = parse_f32(&file(GOOD, &1.5f32.to_le_bytes())).unwrap();

        assert_eq!(values, [1.5]);
    }

    #[test]
    fn rejects_other_files() {
        let cases: [Vec<u8>; 8] = [
            Vec::new(),
            b"not numpy data".to_vec(),
            [MAGIC, &[2, 0], &[0, 0]].concat(),
            file(&GOOD.replace("<f4", "<f8"), &[0; 8]),
            file(&GOOD.replace("False", "True"), &[0; 4]),
            file(&GOOD.replace("(1,)", "(1, 2)"), &[0; 8]),
            file(GOOD, &[0; 3]),
            file(&GOOD.replace("(1,)", "(2,)"), &[0; 4]),
        ];

        for bytes in cases {
            assert!(matches!(parse_f32(&bytes), Err(NpyError::Format(_))));
        }
    }

    #[test]
    fn a_header_longer_than_the_file_is_rejected() {
        let bytes = [MAGIC, &VERSION_1, &[255, 255], b"{}"].concat();

        assert!(matches!(parse_f32(&bytes), Err(NpyError::Format(_))));
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        assert!(matches!(
            read_f32(Path::new("/definitely/not/here.npy")),
            Err(NpyError::Io(_))
        ));
    }
}
