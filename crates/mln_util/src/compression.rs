// Ports util::compress and util::decompress from
// platform/default/src/mln/util/compression.cpp.
//
// One backend per platform: the system libz on Apple, as the C++ links it;
// flate2 elsewhere, with zlib-rs natively and miniz_oxide on wasm32, which the
// png crate depends on anyway. On wasm32, `decompress_async` uses the
// browser's DecompressionStream.

#[cfg(target_vendor = "apple")]
mod libz;
#[cfg(target_vendor = "apple")]
use libz as backend;

#[cfg(not(target_vendor = "apple"))]
mod flate;
#[cfg(not(target_vendor = "apple"))]
use flate as backend;

#[cfg(target_family = "wasm")]
mod browser;

use std::sync::Arc;

/// `util::compress(raw)`: a zlib stream (`windowBits = MAX_WBITS`) at
/// `Z_DEFAULT_COMPRESSION` (level 6), `memLevel` 8, `Z_DEFAULT_STRATEGY`.
///
/// The stream decompresses to the same bytes on every platform, but the
/// compressed bytes (and so their length) depend on the backend.
pub fn compress(raw: &[u8]) -> Result<Vec<u8>, String> {
    backend::compress(raw)
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
/// message; the message here is the backend's.
pub fn decompress(raw: &[u8]) -> Result<Vec<u8>, String> {
    backend::decompress(raw)
}

/// [`decompress`], handing the result to `done`.
///
/// On wasm32 the browser inflates the stream (`DecompressionStream`), and
/// `done` runs later, from the event loop of the calling thread. A stream the
/// browser rejects, which includes one with bytes after its end, goes through
/// [`decompress`] instead, so the result and the error are the same as its.
/// Elsewhere `done` runs before this returns.
pub fn decompress_async(raw: Arc<Vec<u8>>, done: impl FnOnce(Result<Vec<u8>, String>) + 'static) {
    #[cfg(target_family = "wasm")]
    wasm_bindgen_futures::spawn_local(async move {
        let result = match browser::decompress(&raw).await {
            Ok(data) => Ok(data),
            Err(_) => decompress(&raw),
        };
        done(result);
    });
    #[cfg(not(target_family = "wasm"))]
    done(decompress(&raw));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// gzip of `raw()` (Python's gzip module, level 6, mtime 0).
    const GZIP: [u8; 56] = [
        0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xed, 0xc5, 0xc1, 0x11, 0x00, 0x30, 0x04, 0x00,
        0x30, 0xb4, 0xec, 0x3f, 0xb2, 0x39, 0xdc, 0x25, 0x9f, 0x44, 0xd6, 0xfb, 0x3d, 0x21, 0x49, 0x92, 0x24, 0x49,
        0x92, 0x24, 0x49, 0x92, 0x24, 0x49, 0x92, 0x24, 0x49, 0xd2, 0xe5, 0x16, 0xe9, 0x1c, 0xb5, 0x68, 0x10, 0x27,
        0x00, 0x00,
    ];

    fn raw() -> Vec<u8> {
        (0..10_000u32).map(|i| (i % 7) as u8).collect()
    }

    #[test]
    fn round_trip() {
        let raw = raw();
        let compressed = compress(&raw).unwrap();
        // zlib header for the default compression level.
        assert_eq!(&compressed[..2], &[0x78, 0x9C]);
        assert!(compressed.len() < raw.len());
        assert_eq!(decompress(&compressed).unwrap(), raw);
        assert!(decompress(&compressed[..compressed.len() / 2]).is_err());
        let mut trailing = compressed.clone();
        trailing.extend_from_slice(b"garbage");
        assert_eq!(decompress(&trailing).unwrap(), raw);
        assert!(decompress(&[]).is_err());
        assert!(decompress(b"not compressed").is_err());
        assert_eq!(decompress(&compress(&[]).unwrap()).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn gzip() {
        // gzip is detected by its magic bytes.
        assert_eq!(decompress(&GZIP).unwrap(), raw());
        assert!(decompress(&GZIP[..GZIP.len() - 4]).is_err());
    }

    #[test]
    fn decompress_async_answers_before_returning() {
        let result = std::rc::Rc::new(std::cell::RefCell::new(None));
        let slot = result.clone();
        decompress_async(Arc::new(GZIP.to_vec()), move |r| *slot.borrow_mut() = Some(r));
        assert_eq!(result.take().unwrap().unwrap(), raw());
    }
}
