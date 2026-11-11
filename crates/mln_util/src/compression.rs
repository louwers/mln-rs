// Ports util::compress and util::decompress from
// platform/default/src/mln/util/compression.cpp.

use std::io::Write;

/// `util::compress(raw)`: a zlib stream (`windowBits = MAX_WBITS`) at
/// `Z_DEFAULT_COMPRESSION` (level 6), `memLevel` 8, `Z_DEFAULT_STRATEGY`.
///
/// flate2's Rust backend (miniz_oxide) produces a valid zlib stream that
/// decompresses to the same bytes, but not necessarily the same compressed
/// bytes as zlib.
pub fn compress(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    encoder.write_all(raw).map_err(|_| "failed to deflate".to_string())?;
    encoder.finish().map_err(|_| "failed to deflate".to_string())
}

/// `util::is_compressed(v)`: a gzip or zlib header (with one of the four
/// usual zlib compression levels) on more than two bytes. (Added by
/// gtests-storage for the MBTiles and PMTiles file sources.)
pub fn is_compressed(v: &[u8]) -> bool {
    if v.len() > 2 {
        let (byte0, byte1) = (v[0], v[1]);
        if byte0 == 0x1f && byte1 == 0x8b {
            // gzip (rfc1952)
            return true;
        } else if byte0 == 0x78 {
            // zlib (rfc1950)
            return matches!(
                byte1,
                0x01 // 78 01 - No Compression/low
                | 0x5E // 78 5E - Fast Compression
                | 0x9C // 78 9C - Default Compression
                | 0xDA // 78 DA - Best Compression
            );
        }
    }
    false
}

/// `util::decompress(raw, windowBits = CompressionFormat::DETECT)`: inflates a
/// zlib or gzip stream (zlib's header auto-detection, `MAX_WBITS + 32`). Like
/// the C++ loop, bytes after the end of the stream are ignored, and a stream
/// that ends early is an error. C++ throws `std::runtime_error` with zlib's
/// message; the message here is flate2's.
pub fn decompress(raw: &[u8]) -> Result<Vec<u8>, String> {
    if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        // gzip (rfc1952): one member; zlib checks the CRC-32 and size trailer.
        let mut decoder = flate2::bufread::GzDecoder::new(raw);
        let mut result = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut result).map_err(|e| e.to_string())?;
        return Ok(result);
    }

    let mut inflate = flate2::Decompress::new(true);
    let mut result = Vec::new();
    let mut out = [0u8; 16384];
    loop {
        let consumed = inflate.total_in() as usize;
        let before = inflate.total_out();
        let status = inflate
            .decompress(&raw[consumed.min(raw.len())..], &mut out, flate2::FlushDecompress::None)
            .map_err(|e| e.to_string())?;
        let produced = (inflate.total_out() - before) as usize;
        result.extend_from_slice(&out[..produced]);
        match status {
            flate2::Status::StreamEnd => return Ok(result),
            // zlib's inflate returns Z_BUF_ERROR when no progress is possible.
            flate2::Status::BufError => return Err("decompression error".to_string()),
            flate2::Status::Ok => {
                if produced == 0 && inflate.total_in() as usize == consumed {
                    return Err("decompression error".to_string());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    #[test]
    fn round_trip() {
        let raw: Vec<u8> = (0..10_000u32).map(|i| (i % 7) as u8).collect();
        let compressed = compress(&raw).unwrap();
        // zlib header for the default compression level.
        assert_eq!(&compressed[..2], &[0x78, 0x9C]);
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(&compressed[..]).read_to_end(&mut out).unwrap();
        assert_eq!(out, raw);
        assert_eq!(decompress(&compressed).unwrap(), raw);
        assert!(decompress(&compressed[..compressed.len() / 2]).is_err());
        let mut trailing = compressed.clone();
        trailing.extend_from_slice(b"garbage");
        assert_eq!(decompress(&trailing).unwrap(), raw);

        // gzip is detected by its magic bytes.
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
        gzip.write_all(&raw).unwrap();
        let gzip = gzip.finish().unwrap();
        assert_eq!(decompress(&gzip).unwrap(), raw);
        assert!(decompress(&gzip[..gzip.len() - 4]).is_err());
    }
}
