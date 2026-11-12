// Decodes WOFF and WOFF2 font files to the SFNT (TrueType or OpenType) font
// they hold, as FreeType's `sfnt_open_font` does before it reads a face. WOFF
// ports ../maplibre-native/vendor/freetype/src/sfnt/sfwoff.c
// (`woff_open_font`); WOFF2 is the wuff crate's decoder. The C++ core builds
// FreeType with zlib and without Brotli, so it opens WOFF but not WOFF2.
// The WOFF port is under the FreeType License (LICENSE-FreeType).

use std::fmt;

use flate2::{Decompress, FlushDecompress, Status};

use crate::glyph::FontData;

/// Why a WOFF or WOFF2 file didn't decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WoffError {
    /// The WOFF header doesn't match the file, or is inconsistent.
    InvalidHeader,
    /// A WOFF table, or the metadata or private block, isn't where the
    /// header and table directory place it, or the tables aren't sorted by
    /// tag.
    InvalidTableDirectory,
    /// A compressed WOFF table isn't a zlib stream of its original length.
    InvalidTableData,
    /// The decoded font doesn't fit in memory.
    OutOfMemory,
    /// The WOFF2 file is malformed (its decoder gives no detail).
    InvalidWoff2,
}

impl fmt::Display for WoffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            WoffError::InvalidHeader => "invalid WOFF header",
            WoffError::InvalidTableDirectory => "invalid WOFF table directory",
            WoffError::InvalidTableData => "invalid WOFF table data",
            WoffError::OutOfMemory => "the decoded WOFF font doesn't fit in memory",
            WoffError::InvalidWoff2 => "invalid WOFF2 file",
        })
    }
}

impl std::error::Error for WoffError {}

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

const WOFF_HEADER_SIZE: u64 = 44;
const WOFF_TABLE_ENTRY_SIZE: u64 = 20;
const SFNT_HEADER_SIZE: u64 = 12;
const SFNT_TABLE_ENTRY_SIZE: u64 = 16;
/// Deflate's largest ratio of output to input, from the zlib manual.
const MAX_DEFLATE_RATIO: u64 = 1032;

/// `WOFF_TableRec`.
struct Table {
    tag: u32,
    offset: u32,
    comp_length: u32,
    orig_length: u32,
    checksum: u32,
    orig_offset: u64,
}

fn read_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn round4(n: u64) -> u64 {
    (n + 3) & !3
}

/// Decodes a WOFF file (`woff_open_font`): its tables, uncompressed, behind
/// an SFNT header and table directory. The checksums and the metadata aren't
/// checked.
pub fn decode_woff(data: &[u8]) -> Result<Vec<u8>, WoffError> {
    use WoffError::{InvalidHeader, InvalidTableData, InvalidTableDirectory};

    let header = |at| read_u32(data, at).ok_or(InvalidHeader);
    if read_u32(data, 0) != Some(u32::from_be_bytes(*b"wOFF")) {
        return Err(InvalidHeader);
    }
    let flavor = header(4)?;
    let length = u64::from(header(8)?);
    let num_tables = read_u16(data, 12).ok_or(InvalidHeader)?;
    let total_sfnt_size = u64::from(header(16)?);
    let meta_offset = u64::from(header(24)?);
    let meta_length = u64::from(header(28)?);
    let meta_orig_length = u64::from(header(32)?);
    let priv_offset = u64::from(header(36)?);
    let priv_length = u64::from(header(40)?);

    // Make sure we don't recurse back here or hit TTC code.
    if flavor == u32::from_be_bytes(*b"wOFF") || flavor == u32::from_be_bytes(*b"ttcf") {
        return Err(InvalidHeader);
    }
    let n = u64::from(num_tables);
    if length != data.len() as u64
        || num_tables == 0
        || num_tables > 0xFFF
        || WOFF_HEADER_SIZE + n * WOFF_TABLE_ENTRY_SIZE >= length
        || SFNT_HEADER_SIZE + n * SFNT_TABLE_ENTRY_SIZE >= total_sfnt_size
        || total_sfnt_size & 3 != 0
        || (meta_offset == 0 && (meta_length != 0 || meta_orig_length != 0))
        || (meta_length != 0 && meta_orig_length == 0)
        || (priv_offset == 0 && priv_length != 0)
    {
        return Err(InvalidHeader);
    }

    let mut tables = Vec::with_capacity(usize::from(num_tables));
    let mut old_tag = 0;
    for nn in 0..usize::from(num_tables) {
        let at = 44 + nn * 20;
        let field = |i: usize| read_u32(data, at + i * 4).ok_or(InvalidTableDirectory);
        let table = Table {
            tag: field(0)?,
            offset: field(1)?,
            comp_length: field(2)?,
            orig_length: field(3)?,
            checksum: field(4)?,
            orig_offset: 0,
        };
        if table.tag <= old_tag {
            return Err(InvalidTableDirectory);
        }
        old_tag = table.tag;
        tables.push(table);
    }

    // While the entries in the sfnt header must be sorted by the tag value,
    // the tables themselves are not. We thus have to sort them by offset and
    // check that they don't overlap.
    let mut by_offset: Vec<usize> = (0..tables.len()).collect();
    by_offset.sort_by_key(|&i| tables[i].offset);
    let mut woff_offset = WOFF_HEADER_SIZE + n * WOFF_TABLE_ENTRY_SIZE;
    let mut sfnt_offset = SFNT_HEADER_SIZE + n * SFNT_TABLE_ENTRY_SIZE;
    for i in by_offset {
        let table = &mut tables[i];
        let (offset, comp_length, orig_length) =
            (u64::from(table.offset), u64::from(table.comp_length), u64::from(table.orig_length));
        // Beyond FreeType: no zlib stream inflates more than deflate can, so
        // a small file can't claim a huge font.
        if offset != woff_offset
            || comp_length > length
            || offset > length - comp_length
            || orig_length > total_sfnt_size
            || sfnt_offset > total_sfnt_size - orig_length
            || comp_length > orig_length
            || orig_length > comp_length * MAX_DEFLATE_RATIO
        {
            return Err(InvalidTableDirectory);
        }
        table.orig_offset = sfnt_offset;
        // The offsets must be multiples of 4.
        woff_offset += round4(comp_length);
        sfnt_offset += round4(orig_length);
    }

    if meta_offset != 0 {
        if meta_offset != woff_offset || meta_offset + meta_length > length {
            return Err(InvalidTableDirectory);
        }
        // We have padding only ...
        woff_offset += meta_length;
    }
    if priv_offset != 0 {
        // ... if it isn't the last block.
        woff_offset = round4(woff_offset);
        if priv_offset != woff_offset || priv_offset + priv_length > length {
            return Err(InvalidTableDirectory);
        }
        // No padding for the last block.
        woff_offset += priv_length;
    }
    if sfnt_offset != total_sfnt_size || woff_offset != length {
        return Err(InvalidTableDirectory);
    }

    // `total_sfnt_size` is now known to be the tables' padded lengths, each
    // no more than its data in the file can inflate to.
    let total = usize::try_from(total_sfnt_size).map_err(|_| WoffError::OutOfMemory)?;
    let mut sfnt = Vec::new();
    sfnt.try_reserve_exact(total).map_err(|_| WoffError::OutOfMemory)?;
    sfnt.resize(total, 0);

    let entry_selector = 15 - num_tables.leading_zeros() as u16; // FT_MSB
    let search_range = (1u16 << entry_selector) * 16;
    let range_shift = num_tables * 16 - search_range;
    let mut directory = Vec::with_capacity(12 + tables.len() * 16);
    directory.extend_from_slice(&flavor.to_be_bytes());
    directory.extend_from_slice(&num_tables.to_be_bytes());
    directory.extend_from_slice(&search_range.to_be_bytes());
    directory.extend_from_slice(&entry_selector.to_be_bytes());
    directory.extend_from_slice(&range_shift.to_be_bytes());

    for table in &tables {
        directory.extend_from_slice(&table.tag.to_be_bytes());
        directory.extend_from_slice(&table.checksum.to_be_bytes());
        // Below `total_sfnt_size`, a u32.
        directory.extend_from_slice(&(table.orig_offset as u32).to_be_bytes());
        directory.extend_from_slice(&table.orig_length.to_be_bytes());

        // The checks above keep both ranges inside their buffers.
        let source = table.offset as usize..table.offset as usize + table.comp_length as usize;
        let dest = table.orig_offset as usize..table.orig_offset as usize + table.orig_length as usize;
        let (Some(source), Some(dest)) = (data.get(source), sfnt.get_mut(dest)) else {
            return Err(InvalidTableDirectory);
        };
        if table.comp_length == table.orig_length {
            // Uncompressed data; just copy.
            dest.copy_from_slice(source);
        } else {
            // Uncompress with zlib.
            let mut inflater = Decompress::new(true);
            match inflater.decompress(source, dest, FlushDecompress::Finish) {
                Ok(Status::StreamEnd) if inflater.total_out() == u64::from(table.orig_length) => {}
                _ => return Err(InvalidTableData),
            }
        }
        // The padding bytes stay zero.
    }
    let Some(head) = sfnt.get_mut(..directory.len()) else {
        return Err(InvalidHeader);
    };
    head.copy_from_slice(&directory);
    Ok(sfnt)
}
