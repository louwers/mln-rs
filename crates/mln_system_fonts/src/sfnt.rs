//! The tables of a font file's face, and font files made of tables.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// A table's tag and its bytes.
pub(crate) type Table = ([u8; 4], Vec<u8>);

/// The largest table that `read_tables` reads, well above the `cmap` of a
/// CJK font.
const MAX_TABLE_LEN: u32 = 1 << 24;

#[cfg(any(test, target_vendor = "apple"))]
/// A font file with `tables`, in tag order, as the sfnt format requires.
pub(crate) fn build(mut tables: Vec<Table>) -> Vec<u8> {
    tables.sort_by_key(|(tag, _)| *tag);
    let count = tables.len() as u16;
    let cff = tables.iter().any(|(tag, _)| tag == b"CFF " || tag == b"CFF2");
    // searchRange, entrySelector and rangeShift: the largest power of two
    // at most `count`, for a binary search of the records.
    let entry_selector = if count == 0 { 0 } else { 15 - count.leading_zeros() as u16 };
    let search_range = (1u16 << entry_selector) * 16;
    let header_len = 12 + 16 * tables.len();
    let data_len: usize = tables.iter().map(|(_, data)| data.len().next_multiple_of(4)).sum();

    let mut out = Vec::with_capacity(header_len + data_len);
    out.extend_from_slice(if cff { b"OTTO" } else { &[0, 1, 0, 0] });
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&(count * 16).saturating_sub(search_range).to_be_bytes());
    let mut offset = header_len;
    for (tag, data) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(data).to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += data.len().next_multiple_of(4);
    }
    for (_, data) in &tables {
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    out
}

#[cfg(any(test, target_vendor = "apple"))]
/// The sum of a table's big-endian 32-bit words, its last zero-padded.
fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// The tables `tags` of face `index` of the font file at `path`, which it
/// reads without the rest of the file. A tag the face lacks is left out.
pub(crate) fn read_tables(path: &Path, index: u32, tags: &[[u8; 4]]) -> io::Result<Vec<Table>> {
    let mut file = File::open(path)?;
    let mut header = [0; 12];
    file.read_exact(&mut header)?;
    let mut directory = 0;
    if &header[..4] == b"ttcf" {
        if index >= be32(&header[8..]) {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no such face in the collection"));
        }
        let mut offset = [0; 4];
        file.seek(SeekFrom::Start(12 + 4 * u64::from(index)))?;
        file.read_exact(&mut offset)?;
        directory = u64::from(be32(&offset));
        file.seek(SeekFrom::Start(directory))?;
        file.read_exact(&mut header)?;
    } else if index != 0 {
        return Err(io::Error::new(io::ErrorKind::NotFound, "not a collection"));
    }
    let count = usize::from(u16::from_be_bytes([header[4], header[5]]));
    let mut records = vec![0; 16 * count];
    file.seek(SeekFrom::Start(directory + 12))?;
    file.read_exact(&mut records)?;

    let mut tables = Vec::new();
    for record in records.as_chunks::<16>().0 {
        let tag: [u8; 4] = [record[0], record[1], record[2], record[3]];
        if !tags.contains(&tag) {
            continue;
        }
        let (offset, len) = (be32(&record[8..]), be32(&record[12..]));
        if len > MAX_TABLE_LEN {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "table too large"));
        }
        let mut data = vec![0; len as usize];
        file.seek(SeekFrom::Start(u64::from(offset)))?;
        file.read_exact(&mut data)?;
        tables.push((tag, data));
    }
    Ok(tables)
}

fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[cfg(test)]
mod tests {
    use skrifa::MetadataProvider;
    use skrifa::raw::FontRef;

    use super::{build, read_tables};

    /// A font with a `cmap` that maps 'A' to glyph 1, and an `OS/2` of the
    /// given weight.
    fn font(weight: u16) -> Vec<u8> {
        build(vec![(*b"cmap", cmap_a()), (*b"OS/2", os2(weight))])
    }

    /// A format 4 `cmap` that maps U+0041 to glyph 1.
    fn cmap_a() -> Vec<u8> {
        let mut table = Vec::new();
        // version, numTables, then a Windows Unicode BMP record at offset 12.
        for word in [0u16, 1, 3, 1] {
            table.extend_from_slice(&word.to_be_bytes());
        }
        table.extend_from_slice(&12u32.to_be_bytes());
        // Format 4 with segments [0x41, 0x41] (delta 1 - 0x41) and the end
        // segment.
        let words: [u16; 16] = [4, 32, 0, 4, 4, 1, 0, 0x41, 0xFFFF, 0, 0x41, 0xFFFF, 1u16.wrapping_sub(0x41), 1, 0, 0];
        for word in words {
            table.extend_from_slice(&word.to_be_bytes());
        }
        table
    }

    fn os2(weight: u16) -> Vec<u8> {
        let mut table = vec![0; 78];
        table[4..6].copy_from_slice(&weight.to_be_bytes());
        table
    }

    /// A collection of `fonts`, sharing nothing.
    fn collection(fonts: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"ttcf".to_vec();
        out.extend_from_slice(&[0, 1, 0, 0]);
        out.extend_from_slice(&(fonts.len() as u32).to_be_bytes());
        let mut offset = 12 + 4 * fonts.len();
        let mut offsets = Vec::new();
        for font in fonts {
            offsets.push(offset);
            offset += font.len();
        }
        for offset in &offsets {
            out.extend_from_slice(&(*offset as u32).to_be_bytes());
        }
        // Each font's table offsets are from the start of its own file,
        // so move them by its place in the collection.
        for (font, base) in fonts.iter().zip(offsets) {
            let mut font = font.clone();
            let count = usize::from(u16::from_be_bytes([font[4], font[5]]));
            for i in 0..count {
                let at = 12 + 16 * i + 8;
                let moved = super::be32(&font[at..]) + base as u32;
                font[at..at + 4].copy_from_slice(&moved.to_be_bytes());
            }
            out.extend_from_slice(&font);
        }
        out
    }

    fn weight(data: &[u8]) -> u16 {
        use skrifa::raw::TableProvider;
        FontRef::new(data).unwrap().os2().unwrap().us_weight_class()
    }

    #[test]
    fn a_built_font_reads_back() {
        let data = font(400);
        let font = FontRef::new(&data).expect("a font");
        assert_eq!(font.charmap().map('A').map(u32::from), Some(1));
        assert_eq!(weight(&data), 400);
        assert_eq!(&data[..4], &[0, 1, 0, 0]);
    }

    #[test]
    fn tables_read_from_a_file_face() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fonts.ttc");
        std::fs::write(&path, collection(&[font(100), font(400)])).unwrap();
        let tables = read_tables(&path, 1, &[*b"OS/2"]).unwrap();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].1, os2(400));
        assert!(read_tables(&path, 2, &[*b"OS/2"]).is_err());

        let path = dir.path().join("font.ttf");
        std::fs::write(&path, font(400)).unwrap();
        let tables = read_tables(&path, 0, &[*b"cmap", *b"fvar"]).unwrap();
        assert_eq!(tables, vec![(*b"cmap", cmap_a())]);
    }
}
