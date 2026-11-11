//! The image codecs: ports of those `include/mln/util/image.hpp` declares
//! (`decodeImage`, `encodePNG`), from
//! `platform/default/src/mln/util/{image,jpeg_reader,png_reader,png_writer}.cpp`.
//!
//! C++ throws `std::runtime_error`; here the functions return
//! `Err(message)` with the same message.
//!
//! Port-only: `decoder::decode` is how the map's code decodes images, with
//! `decode_image` unless the host sets a decoder of its own (the browser's).

#[cfg(target_family = "wasm")]
mod browser_decoder;
pub mod decoder;
pub mod image;
#[cfg(not(target_family = "wasm"))]
pub mod jpeg_reader;
pub mod png_reader;
pub mod png_writer;
pub mod webp_reader;

pub use image::decode_image;
pub use png_reader::decode_png;
pub use png_writer::encode_png;
