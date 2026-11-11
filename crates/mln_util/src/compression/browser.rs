//! Decompression by the browser's `DecompressionStream`.

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
    /// `DecompressionStream`, which web-sys has only behind its unstable APIs.
    type DecompressionStream;

    #[wasm_bindgen(constructor, catch)]
    fn new(format: &str) -> Result<DecompressionStream, JsValue>;

    /// A `ReadableStream`, with the `pipeThrough` that web-sys only has with
    /// its `ReadableWritablePair` feature.
    #[wasm_bindgen(js_name = ReadableStream)]
    type Stream;

    #[wasm_bindgen(method, js_name = pipeThrough)]
    fn pipe_through(this: &Stream, transform: &DecompressionStream) -> web_sys::ReadableStream;
}

/// Inflates a gzip stream (by its magic bytes) or a zlib one. Rejects bytes
/// after the end of the stream and a stream that ends early.
pub(super) async fn decompress(raw: &[u8]) -> Result<Vec<u8>, JsValue> {
    // The Compression Streams spec's "deflate" is the zlib format.
    let format = if raw.starts_with(&[0x1f, 0x8b]) { "gzip" } else { "deflate" };
    let transform = DecompressionStream::new(format)?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(raw));
    let input = web_sys::Blob::new_with_u8_array_sequence(&parts)?;
    let output = input.stream().unchecked_into::<Stream>().pipe_through(&transform);
    let response = web_sys::Response::new_with_opt_readable_stream(Some(&output))?;
    let buffer = JsFuture::from(response.array_buffer()?).await?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}
