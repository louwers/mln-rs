//! Images: the codecs, ports of those `include/mln/util/image.hpp` declares
//! (`decodeImage`, `encodePNG`), from
//! `platform/default/src/mln/util/{image,jpeg_reader,png_reader,png_writer}.cpp`.
//!
//! C++ throws `std::runtime_error`; here the functions return
//! `Err(message)` with the same message.
//!
//! Port-only: `decoder::decode` is how the map's code decodes images: with
//! `decode_image` natively, and with the browser's decoders on wasm.
//!
//! The style's images, as the renderer's layouts request them, are in
//! `image_store`, and the sprite parser in [`sprite`].

#[cfg(target_family = "wasm")]
mod browser_decoder;
pub mod decoder;
#[expect(clippy::module_inception, reason = "`image::image` holds `decode_image`")]
mod image;
pub(crate) mod image_store;
#[cfg(not(target_family = "wasm"))]
mod jpeg_reader;
mod png_reader;
pub mod png_writer;
pub mod premultiply;
pub mod sprite;
mod webp_reader;

pub use image::decode_image;
pub use png_reader::decode_png;
pub use png_writer::encode_png;
