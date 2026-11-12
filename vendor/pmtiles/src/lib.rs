//! Ports the reading half of vendor/PMTiles/cpp/pmtiles.hpp (PMTiles v3,
//! BSD-3-Clause, Protomaps LLC): `headerv3` and `deserialize_header`,
//! `entryv3` and `deserialize_directory`, `find_tile`, `zxy_to_tileid` and
//! `tileid_to_zxy`. The exceptions are `Error`s with the same `what()`.
//!
//! TODO(port): the writing half (`serialize`, `serialize_directory`,
//! `build_root_leaves`, `make_root_leaves`), `collect_entries`,
//! `entries_tms` and `get_tile` over a memory map (unused by the core).

use std::cmp::Ordering;

pub const TILETYPE_UNKNOWN: u8 = 0x0;
pub const TILETYPE_MVT: u8 = 0x1;
pub const TILETYPE_PNG: u8 = 0x2;
pub const TILETYPE_JPEG: u8 = 0x3;
pub const TILETYPE_WEBP: u8 = 0x4;
pub const TILETYPE_AVIF: u8 = 0x5;

pub const COMPRESSION_UNKNOWN: u8 = 0x0;
pub const COMPRESSION_NONE: u8 = 0x1;
pub const COMPRESSION_GZIP: u8 = 0x2;
pub const COMPRESSION_BROTLI: u8 = 0x3;
pub const COMPRESSION_ZSTD: u8 = 0x4;

/// The exceptions of pmtiles.hpp; `Display` is `what()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// `pmtiles_magic_number_exception`
    MagicNumber,
    /// `pmtiles_version_exception`
    Version,
    /// `varint_too_long_exception`
    VarintTooLong,
    /// `end_of_buffer_exception`
    EndOfBuffer,
    /// `malformed_directory_exception`
    MalformedDirectory,
    /// `std::overflow_error`
    Overflow(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Error::MagicNumber => "pmtiles magic number exception",
            Error::Version => "pmtiles version: must be 3",
            Error::VarintTooLong => "varint too long exception",
            Error::EndOfBuffer => "end of buffer exception",
            Error::MalformedDirectory => "malformed directory exception",
            Error::Overflow(message) => message,
        })
    }
}

impl std::error::Error for Error {}

/// `headerv3`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeaderV3 {
    pub root_dir_offset: u64,
    pub root_dir_bytes: u64,
    pub json_metadata_offset: u64,
    pub json_metadata_bytes: u64,
    pub leaf_dirs_offset: u64,
    pub leaf_dirs_bytes: u64,
    pub tile_data_offset: u64,
    pub tile_data_bytes: u64,
    pub addressed_tiles_count: u64,
    pub tile_entries_count: u64,
    pub tile_contents_count: u64,
    pub clustered: bool,
    pub internal_compression: u8,
    pub tile_compression: u8,
    pub tile_type: u8,
    pub min_zoom: u8,
    pub max_zoom: u8,
    pub min_lon_e7: i32,
    pub min_lat_e7: i32,
    pub max_lon_e7: i32,
    pub max_lat_e7: i32,
    pub center_zoom: u8,
    pub center_lon_e7: i32,
    pub center_lat_e7: i32,
}

fn u64_at(s: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(s[offset..offset + 8].try_into().unwrap())
}

fn i32_at(s: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(s[offset..offset + 4].try_into().unwrap())
}

/// `deserialize_header(const std::string&)`.
pub fn deserialize_header(s: &[u8]) -> Result<HeaderV3, Error> {
    if s.get(..7) != Some(b"PMTiles".as_slice()) {
        // `s.substr(0, 7) != "PMTiles"` (a shorter string differs too).
        return Err(Error::MagicNumber);
    }
    if s.len() != 127 || s[7] != 0x3 {
        return Err(Error::Version);
    }
    Ok(HeaderV3 {
        root_dir_offset: u64_at(s, 8),
        root_dir_bytes: u64_at(s, 16),
        json_metadata_offset: u64_at(s, 24),
        json_metadata_bytes: u64_at(s, 32),
        leaf_dirs_offset: u64_at(s, 40),
        leaf_dirs_bytes: u64_at(s, 48),
        tile_data_offset: u64_at(s, 56),
        tile_data_bytes: u64_at(s, 64),
        addressed_tiles_count: u64_at(s, 72),
        tile_entries_count: u64_at(s, 80),
        tile_contents_count: u64_at(s, 88),
        clustered: s[96] == 0x1,
        internal_compression: s[97],
        tile_compression: s[98],
        tile_type: s[99],
        min_zoom: s[100],
        max_zoom: s[101],
        min_lon_e7: i32_at(s, 102),
        min_lat_e7: i32_at(s, 106),
        max_lon_e7: i32_at(s, 110),
        max_lat_e7: i32_at(s, 114),
        center_zoom: s[118],
        center_lon_e7: i32_at(s, 119),
        center_lat_e7: i32_at(s, 123),
    })
}

/// `zxy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zxy {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

/// `entryv3`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EntryV3 {
    pub tile_id: u64,
    pub offset: u64,
    pub length: u32,
    pub run_length: u32,
}

const MAX_VARINT_LENGTH: usize = 64 / 7 + 1;

/// `decode_varint_impl` (protozero's): `data` advances past the varint.
// The fast path's `loop` is protozero's `do { ... } while (false)`.
#[allow(clippy::never_loop)]
fn decode_varint_impl(data: &mut &[u8]) -> Result<u64, Error> {
    let bytes = *data;
    let mut p = 0;
    let mut val: u64 = 0;

    if bytes.len() >= MAX_VARINT_LENGTH {
        // fast path
        loop {
            for shift in (0..63).step_by(7) {
                let b = bytes[p] as i8;
                p += 1;
                val |= (u64::from(b as u8) & 0x7f) << shift;
                if b >= 0 {
                    *data = &bytes[p..];
                    return Ok(val);
                }
            }
            let b = bytes[p] as i8;
            p += 1;
            val |= (u64::from(b as u8) & 0x01) << 63;
            if b >= 0 {
                break;
            }
            return Err(Error::VarintTooLong);
        }
    } else {
        let mut shift = 0u32;
        while p != bytes.len() && (bytes[p] as i8) < 0 {
            val |= (u64::from(bytes[p]) & 0x7f).wrapping_shl(shift);
            p += 1;
            shift += 7;
        }
        if p == bytes.len() {
            return Err(Error::EndOfBuffer);
        }
        val |= u64::from(bytes[p]).wrapping_shl(shift);
        p += 1;
    }

    *data = &bytes[p..];
    Ok(val)
}

/// `decode_varint`.
fn decode_varint(data: &mut &[u8]) -> Result<u64, Error> {
    // If this is a one-byte varint, decode it here.
    if let Some(&b) = data.first()
        && b & 0x80 == 0
    {
        *data = &data[1..];
        return Ok(u64::from(b));
    }
    // If this varint is more than one byte, defer to complete implementation.
    decode_varint_impl(data)
}

fn rotate(n: i64, x: &mut u32, y: &mut u32, rx: u32, ry: u32) {
    if ry == 0 {
        if rx != 0 {
            *x = (n - 1 - i64::from(*x)) as u32;
            *y = (n - 1 - i64::from(*y)) as u32;
        }
        std::mem::swap(x, y);
    }
}

/// `find_tile`: use a 0 length entry as a null value.
#[must_use]
pub fn find_tile(entries: &[EntryV3], tile_id: u64) -> EntryV3 {
    let mut m: i64 = 0;
    let mut n: i64 = entries.len() as i64 - 1;
    while m <= n {
        let k = (n + m) >> 1;
        let entry = &entries[k as usize];
        match tile_id.cmp(&entry.tile_id) {
            Ordering::Greater => m = k + 1,
            Ordering::Less => n = k - 1,
            Ordering::Equal => return *entry,
        }
    }

    if n >= 0 {
        let entry = &entries[n as usize];
        if entry.run_length == 0 {
            return *entry;
        }
        if tile_id - entry.tile_id < u64::from(entry.run_length) {
            return *entry;
        }
    }

    EntryV3::default()
}

/// `tileid_to_zxy`.
pub fn tileid_to_zxy(tileid: u64) -> Result<Zxy, Error> {
    if tileid > 6_148_914_691_236_517_204 {
        return Err(Error::Overflow("tile zoom exceeds 64-bit limit"));
    }
    let bit_width = 64 - (3 * tileid + 1).leading_zeros() as u8;
    let z = (bit_width - 1) / 2;
    let acc = ((1i64 << (z * 2)) - 1) as u64 / 3;
    let mut pos = tileid - acc;
    let (mut x, mut y) = (0u32, 0u32);
    for a in 0..z {
        let s: u64 = 1 << a;
        let rx = (s & (pos / 2)) as u32;
        let ry = (s & (pos ^ u64::from(rx))) as u32;
        rotate(s as i64, &mut x, &mut y, rx, ry);
        pos >>= 1;
        x = x.wrapping_add(rx);
        y = y.wrapping_add(ry);
    }
    Ok(Zxy { z, x, y })
}

/// `zxy_to_tileid`.
pub fn zxy_to_tileid(z: u8, x: u32, y: u32) -> Result<u64, Error> {
    if z > 31 {
        return Err(Error::Overflow("tile zoom exceeds 64-bit limit"));
    }
    let max = (1u32 << z).wrapping_sub(1);
    if x > max || y > max {
        return Err(Error::Overflow("tile x/y outside zoom level bounds"));
    }
    let mut acc: u64 = (((1i64 << (u32::from(z) * 2)) - 1) / 3) as u64;
    let (mut tx, mut ty) = (x, y);
    let mut a: i32 = i32::from(z) - 1;
    // `for (uint32_t s = 1LL << a; s > 0; s >>= 1)`: for z = 0, `1LL << -1`
    // is undefined; x86-64 shifts by 63 & 63, and the truncation to uint32_t
    // gives 0, so the loop doesn't run.
    let mut s: u32 = if a < 0 { 0 } else { 1u32 << a };
    while s > 0 {
        let rx = s & tx;
        let ry = s & ty;
        rotate(i64::from(s), &mut tx, &mut ty, rx, ry);
        acc = acc.wrapping_add(((3u64 * u64::from(rx)) ^ u64::from(ry)) << a);
        a -= 1;
        s >>= 1;
    }
    Ok(acc)
}

/// `deserialize_directory`: takes an uncompressed byte buffer.
pub fn deserialize_directory(decompressed: &[u8]) -> Result<Vec<EntryV3>, Error> {
    let mut t = decompressed;

    let num_entries_64bit = decode_varint(&mut t)?;
    // Sanity check to avoid excessive memory allocation attempt: each
    // directory entry takes at least 4 bytes
    if num_entries_64bit / 4 > decompressed.len() as u64 {
        return Err(Error::MalformedDirectory);
    }
    let num_entries = num_entries_64bit as usize;

    let mut result = vec![EntryV3::default(); num_entries];

    let mut last_id: u64 = 0;
    for entry in &mut result {
        let val = decode_varint(&mut t)?;
        if val > u64::MAX - last_id {
            return Err(Error::MalformedDirectory);
        }
        let tile_id = last_id + val;
        entry.tile_id = tile_id;
        last_id = tile_id;
    }

    for entry in &mut result {
        let val = decode_varint(&mut t)?;
        if val > u64::from(u32::MAX) {
            return Err(Error::MalformedDirectory);
        }
        entry.run_length = val as u32;
    }

    for entry in &mut result {
        let val = decode_varint(&mut t)?;
        if val > u64::from(u32::MAX) {
            return Err(Error::MalformedDirectory);
        }
        entry.length = val as u32;
    }

    for i in 0..num_entries {
        let tmp = decode_varint(&mut t)?;

        if i > 0 && tmp == 0 {
            let previous = result[i - 1];
            if previous.offset > u64::MAX - u64::from(previous.length) {
                return Err(Error::MalformedDirectory);
            }
            result[i].offset = previous.offset + u64::from(previous.length);
        } else {
            result[i].offset = tmp.wrapping_sub(1);
        }
    }

    // assert the directory has been fully consumed
    if !t.is_empty() {
        return Err(Error::MalformedDirectory);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_ids_round_trip() {
        assert_eq!(0, zxy_to_tileid(0, 0, 0).unwrap());
        assert_eq!(1, zxy_to_tileid(1, 0, 0).unwrap());
        assert_eq!(2, zxy_to_tileid(1, 0, 1).unwrap());
        assert_eq!(3, zxy_to_tileid(1, 1, 1).unwrap());
        assert_eq!(4, zxy_to_tileid(1, 1, 0).unwrap());
        assert_eq!(5, zxy_to_tileid(2, 0, 0).unwrap());
        for z in 0..8u8 {
            for x in 0..(1u32 << z) {
                for y in 0..(1u32 << z) {
                    let id = zxy_to_tileid(z, x, y).unwrap();
                    assert_eq!(Zxy { z, x, y }, tileid_to_zxy(id).unwrap());
                }
            }
        }
        assert!(zxy_to_tileid(32, 0, 0).is_err());
        assert!(zxy_to_tileid(2, 4, 0).is_err());
    }

    #[test]
    fn varints() {
        let mut data: &[u8] = &[0xac, 0x02];
        assert_eq!(300, decode_varint(&mut data).unwrap());
        assert_eq!(data, []);
        let mut data: &[u8] = &[0xac];
        assert_eq!(Err(Error::EndOfBuffer), decode_varint(&mut data));
        let mut data: &[u8] = &[0xff; 12];
        assert_eq!(Err(Error::VarintTooLong), decode_varint(&mut data));
    }

    #[test]
    fn directory() {
        // 2 entries: ids 0 and 1, run lengths 1, lengths 10 and 20, offsets
        // 0 (written as 1) and contiguous (0).
        let dir = [2, 0, 1, 1, 1, 10, 20, 1, 0];
        let entries = deserialize_directory(&dir).unwrap();
        assert_eq!(
            vec![
                EntryV3 { tile_id: 0, offset: 0, length: 10, run_length: 1 },
                EntryV3 { tile_id: 1, offset: 10, length: 20, run_length: 1 }
            ],
            entries
        );
        assert_eq!(entries[1], find_tile(&entries, 1));
        assert_eq!(EntryV3::default(), find_tile(&entries, 5));
        assert_eq!(Err(Error::MalformedDirectory), deserialize_directory(&[2, 0, 1, 1, 1, 10, 20, 1, 0, 0]));
    }
}
