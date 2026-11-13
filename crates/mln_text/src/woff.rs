// Decodes WOFF and WOFF2 font files to the SFNT (TrueType or OpenType) font
// they hold, as FreeType's `sfnt_open_font` does before it reads a face. The
// wuff crate decodes both; WOFF's zlib streams inflate through flate2, which
// the rest of the workspace uses too.

use std::error::Error;

use flate2::{Decompress, FlushDecompress, Status};

use crate::glyph::FontData;

/// Why a WOFF or WOFF2 file didn't decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WoffError {
    /// The WOFF file is malformed (its decoder gives no detail).
    #[error("invalid WOFF file")]
    InvalidWoff,
    /// The WOFF2 file is malformed (its decoder gives no detail).
    #[error("invalid WOFF2 file")]
    InvalidWoff2,
}

/// Whether `data` starts with the WOFF or WOFF2 signature.
#[must_use]
pub fn is_woff(data: &[u8]) -> bool {
    matches!(data.get(..4), Some(b"wOFF" | b"wOF2"))
}

/// `data` as an SFNT font: decoded when it is a WOFF or WOFF2 file, else
/// `data` itself, uncopied and unchecked.
pub fn to_sfnt(data: FontData) -> Result<FontData, WoffError> {
    match data.as_bytes().get(..4) {
        Some(b"wOFF") => decode_woff(data.as_bytes()).map(FontData::from),
        Some(b"wOF2") => decode_woff2(data.as_bytes()).map(FontData::from),
        _ => Ok(data),
    }
}

/// Decodes a WOFF2 file. A collection decodes to a TrueType collection.
pub fn decode_woff2(data: &[u8]) -> Result<Vec<u8>, WoffError> {
    wuff::decompress_woff2(data).map_err(|_| WoffError::InvalidWoff2)
}

/// Decodes a WOFF file: its tables, uncompressed, behind an SFNT header and
/// table directory. The checksums and the metadata aren't checked.
pub fn decode_woff(data: &[u8]) -> Result<Vec<u8>, WoffError> {
    if !tables_are_disjoint(data) {
        return Err(WoffError::InvalidWoff);
    }
    wuff::decompress_woff1_with_custom_z(data, &mut inflate).map_err(|_| WoffError::InvalidWoff)
}

/// Whether the table directory of WOFF file `data` reads, with no two
/// tables' data overlapping. wuff checks that each table's data is in the
/// file but not this, and with it the decoded font is no larger than the
/// file's tables inflate to: tables sharing one zlib stream could otherwise
/// repeat it into a huge font.
fn tables_are_disjoint(data: &[u8]) -> bool {
    let read_u32 = |at: usize| Some(u64::from(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?)));
    let Some(&[high, low]) = data.get(12..14) else {
        return false;
    };
    let ranges: Option<Vec<(u64, u64)>> = (0..usize::from(u16::from_be_bytes([high, low])))
        .map(|i| {
            let entry = 44 + i * 20;
            let offset = read_u32(entry + 4)?;
            Some((offset, offset + read_u32(entry + 8)?))
        })
        .collect();
    let Some(mut ranges) = ranges else {
        return false;
    };
    ranges.sort_unstable();
    ranges.windows(2).all(|pair| pair[0].1 <= pair[1].0)
}

/// Deflate's largest ratio of output to input, from the zlib manual.
const MAX_DEFLATE_RATIO: usize = 1032;

/// Inflates a WOFF table's zlib stream, which must hold exactly `length`
/// bytes. A `length` beyond what the stream can inflate to fails before
/// anything is allocated, so a small file can't claim a huge font.
fn inflate(compressed: &[u8], length: usize) -> Result<Vec<u8>, Box<dyn Error>> {
    if length > compressed.len().saturating_mul(MAX_DEFLATE_RATIO) {
        return Err("table longer than its zlib stream can inflate to".into());
    }
    let mut table = Vec::new();
    table.try_reserve_exact(length)?;
    let mut inflater = Decompress::new(true);
    match inflater.decompress_vec(compressed, &mut table, FlushDecompress::Finish)? {
        Status::StreamEnd if table.len() == length => Ok(table),
        _ => Err("zlib stream not of the table's length".into()),
    }
}
