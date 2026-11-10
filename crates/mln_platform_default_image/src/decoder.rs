// Port-only: the map's image decoding, behind a decoder that a host can set.
//
// C++ links one `decodeImage` per platform (platform/default's libpng,
// libjpeg and libwebp readers, darwin's CoreGraphics, android's
// BitmapFactory) and calls it where an image arrives, synchronously: in the
// raster and raster-dem tile workers, the sprite parser and image sources.
// Here those call `decode`, which hands the bytes to the decoder that
// `set_decoder` set, or else decodes them with `decode_image` (image.rs) on
// the calling thread before it returns. A host whose decoder answers later
// sets one: the browser's (`createImageBitmap`, platform/web), which keeps
// the Rust decoders out of its wasm module and answers from its event loop,
// on the thread that asked.

use std::sync::{Arc, Mutex};

use mln_util::image::PremultipliedImage;
use mln_util::sync::MutexExt;

use crate::image::{ImageType, UNSUPPORTED, image_type};

/// What `decode_image` returns.
pub type DecodeResult = Result<PremultipliedImage, String>;

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

static DECODER: Mutex<Option<Arc<dyn ImageDecoder>>> = Mutex::new(None);

/// Sets the decoder `decode` uses from now on, instead of the default
/// (`decode_image` on the calling thread): for a host with decoders of its
/// own, such as a browser's.
pub fn set_decoder(decoder: Arc<dyn ImageDecoder>) {
    *DECODER.lock_unpoisoned() = Some(decoder);
}

/// `decodeImage(data)`, handing the result to `done`: before returning,
/// with the default decoder, or later on the calling thread, with a
/// decoder that `set_decoder` set. Data of no type `decode_image`
/// recognizes fails before returning, with its error.
pub fn decode(data: &[u8], done: impl FnOnce(DecodeResult) + 'static) {
    let Some(kind) = image_type(data) else {
        done(Err(UNSUPPORTED.to_string()));
        return;
    };
    let decoder = DECODER.lock_unpoisoned().clone();
    match decoder {
        Some(decoder) => decoder.decode(data, kind, Box::new(done)),
        None => default_decode(data, kind, done),
    }
}

#[cfg(not(target_family = "wasm"))]
fn default_decode(data: &[u8], _kind: ImageType, done: impl FnOnce(DecodeResult)) {
    done(crate::image::decode_image(data));
}

/// The Rust decoders stay out of a wasm module that doesn't call
/// `decode_image` itself, so images fail until a host sets a decoder.
#[cfg(target_family = "wasm")]
fn default_decode(_data: &[u8], kind: ImageType, done: impl FnOnce(DecodeResult)) {
    done(Err(format!("no image decoder for {kind:?} (see decoder::set_decoder)")));
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
    fn default_decoder_answers_at_once() {
        assert_eq!(decode_now(b"GIF89a").unwrap_err(), UNSUPPORTED);
        let png =
            crate::encode_png(&PremultipliedImage::from_vec(mln_util::size::Size::new(1, 1), vec![64, 0, 0, 128]));
        let image = decode_now(&png).unwrap();
        assert_eq!(image.data, vec![64, 0, 0, 128]);
    }
}
