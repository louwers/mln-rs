// Port-only: the map's image decoding.
//
// C++ links one `decodeImage` per platform and calls it synchronously where
// an image arrives. Here those places call `decode`, which hands the bytes to
// the platform's decoder: `decode_image` on the calling thread natively, and
// the browser's on wasm, which answers later from the event loop, on the
// thread that asked.

use mln_util::error::Error;
use mln_util::image::PremultipliedImage;

use crate::image::{ImageType, UNSUPPORTED, image_type};

/// What `decode_image` returns.
pub type DecodeResult = Result<PremultipliedImage, Error>;

/// Where a decoder's result goes.
pub type DecodeDone = Box<dyn FnOnce(DecodeResult)>;

/// A platform's image decoder.
pub trait ImageDecoder: Send + Sync {
    /// Decodes `data`, an image of type `kind`, as `decode_image` does
    /// (premultiplied RGBA8, a WebP that can't be decoded an empty image),
    /// and calls `done` with the result exactly once: before returning, or
    /// later on the calling thread.
    fn decode(&self, data: &[u8], kind: ImageType, done: DecodeDone);
}

/// Decodes `data`, handing the result to `done`: before returning, or later
/// on the calling thread. Data of no type `decode_image` recognizes fails
/// before returning, with its error.
pub fn decode(data: &[u8], done: impl FnOnce(DecodeResult) + 'static) {
    let Some(kind) = image_type(data) else {
        done(Err(Error::unsupported(UNSUPPORTED)));
        return;
    };
    platform_decode(data, kind, done);
}

#[cfg(not(target_family = "wasm"))]
fn platform_decode(data: &[u8], _kind: ImageType, done: impl FnOnce(DecodeResult)) {
    done(crate::image::decode_image(data));
}

#[cfg(target_family = "wasm")]
fn platform_decode(data: &[u8], kind: ImageType, done: impl FnOnce(DecodeResult) + 'static) {
    crate::browser_decoder::BrowserDecoder.decode(data, kind, Box::new(done));
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    fn decode_now(data: &[u8]) -> DecodeResult {
        let result = Rc::new(RefCell::new(None));
        let slot = result.clone();
        decode(data, move |r| *slot.borrow_mut() = Some(r));
        result.take().expect("the default decoder answers before returning")
    }

    #[test]
    fn native_decoder_answers_at_once() {
        assert_eq!(decode_now(b"GIF89a").unwrap_err().message, UNSUPPORTED);
        let png =
            crate::encode_png(&PremultipliedImage::from_vec(mln_util::size::Size::new(1, 1), vec![64, 0, 0, 128]));
        let image = decode_now(&png).unwrap();
        assert_eq!(image.data, vec![64, 0, 0, 128]);
    }
}
