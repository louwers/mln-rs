//! `compress` and `decompress` on flate2: zlib-rs natively, miniz_oxide on
//! wasm32. Error messages are flate2's.

use std::io::Write;

pub(super) fn compress(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    encoder.write_all(raw).map_err(|_| "failed to deflate".to_string())?;
    encoder.finish().map_err(|_| "failed to deflate".to_string())
}

pub(super) fn decompress(raw: &[u8]) -> Result<Vec<u8>, String> {
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
