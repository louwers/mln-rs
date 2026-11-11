//! The browser's decoders as the `ImageDecoder`, so that the Rust PNG and
//! WebP decoders stay out of the wasm module.
//!
//! `createImageBitmap` decodes without premultiplying or converting colours,
//! as libpng and libwebp decode for C++, and its straight RGBA8 texels are
//! read back through a `VideoFrame` or a canvas (`read_bitmap`), then
//! premultiplied as the Rust decoders premultiply them. The decoding and the
//! readback are Orbis's (github.com/sargunv/orbis, crates/orb-web's host).

use js_sys::{Reflect, Uint8Array};
use mln_util::error::Error;
use mln_util::image::{PremultipliedImage, UnassociatedImage};
use mln_util::logging::{Event, Log};
use mln_util::premultiply::premultiply;
use mln_util::size::Size;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::decoder::{DecodeDone, ImageDecoder};
use crate::image::ImageType;

#[wasm_bindgen]
extern "C" {
    /// The global `createImageBitmap`, a window's or a worker's.
    #[wasm_bindgen(js_name = createImageBitmap, catch)]
    fn create_image_bitmap(image: &web_sys::Blob, options: &js_sys::Object) -> Result<js_sys::Promise, JsValue>;
}

pub(crate) struct BrowserDecoder;

impl ImageDecoder for BrowserDecoder {
    fn decode(&self, data: &[u8], kind: ImageType, done: DecodeDone) {
        // The blob copies the bytes now; the caller keeps its own.
        let blob = web_sys::Blob::new_with_u8_array_sequence(&js_sys::Array::of1(&Uint8Array::from(data)));
        wasm_bindgen_futures::spawn_local(async move {
            let image = match blob {
                Ok(blob) => decode_image(&blob).await,
                Err(error) => Err(error),
            };
            done(match image {
                Ok(image) => Ok(premultiply(image)),
                // `decodeWEBP`: an empty image, and a warning.
                Err(_) if kind == ImageType::Webp => {
                    Log::warning(Event::Image, "Failed to decode WebP image contents!");
                    Ok(PremultipliedImage::default())
                }
                Err(error) => {
                    let error = error.as_string().unwrap_or_else(|| format!("{error:?}"));
                    Err(Error::invalid_argument(match kind {
                        // `decodePNG`'s prefix.
                        ImageType::Png => format!("failed to read invalid png: '{error}'"),
                        _ => format!("failed to read invalid {kind:?} image: '{error}'"),
                    }))
                }
            });
        });
    }
}

/// Decodes an image to straight RGBA8 texels.
async fn decode_image(blob: &web_sys::Blob) -> Result<UnassociatedImage, JsValue> {
    let bitmap = decode_bitmap(blob).await?;
    let image = read_bitmap(&bitmap).await;
    bitmap.close();
    image
}

/// Decodes an image into an `ImageBitmap` with straight alpha and no colour
/// conversion, on the page or in a worker.
async fn decode_bitmap(blob: &web_sys::Blob) -> Result<web_sys::ImageBitmap, JsValue> {
    let options = js_sys::Object::new();
    Reflect::set(&options, &"premultiplyAlpha".into(), &"none".into())?;
    Reflect::set(&options, &"colorSpaceConversion".into(), &"none".into())?;
    JsFuture::from(create_image_bitmap(blob, &options)?).await?.dyn_into()
}

/// Reads a bitmap's straight RGBA8 texels: through a `VideoFrame` where
/// this context reads one exactly, and otherwise through a canvas.
///
/// A canvas's readback isn't exact. It keeps texels premultiplied, so
/// translucent ones lose precision, and browsers that resist
/// fingerprinting (Brave) perturb what `getImageData` returns, which turns
/// a DEM tile's texels into spikes. GL JS reads through a `VideoFrame` once
/// a probe finds the canvas distorted (its #3185); a frame is exact and, in
/// Chrome, faster than the canvas, so here it's the first choice wherever
/// it reads exactly.
async fn read_bitmap(bitmap: &web_sys::ImageBitmap) -> Result<UnassociatedImage, JsValue> {
    let size = Size::new(bitmap.width(), bitmap.height());
    if let Ok(pixels) = JsFuture::from(frames::read_frame(bitmap)).await
        && let Some(pixels) = pixels.dyn_ref::<Uint8Array>()
    {
        return Ok(UnassociatedImage::from_vec(size, pixels.to_vec()));
    }
    read_canvas(bitmap)
}

mod frames {
    use wasm_bindgen::prelude::wasm_bindgen;

    // `readFrame(bitmap)` resolves to the bitmap's RGBA8 texels, read
    // through a `VideoFrame`, or to nothing where this context's frames
    // don't read exactly: the first call finds out, decoding a 4 × 4 PNG
    // whose texel bytes count up by 37 from 11 (alphas among them a canvas
    // loses precision at) and reading it back. A frame is copied in its own
    // format (RGBA or BGRA, X for opaque), as GL JS copies it: copying with
    // `format: "RGBA"` converts, and isn't exact.
    #[wasm_bindgen(inline_js = r#"
const PROBE = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+AAAAT0lEQVR42gFEALv/AAswVXqfxOkOM1h9osfsETYAW4Clyu8UOV6DqM3yFzxhhgCr0PUaP2SJrtP4HUJnjLHWAPsgRWqPtNn+I0htkrfcASYmMCAhKZAw6QAAAABJRU5ErkJggg==";
let exact;
async function read(bitmap) {
  const frame = new VideoFrame(bitmap, { timestamp: 0 });
  try {
    const { width, height } = bitmap;
    const format = frame.format ?? "";
    const bgr = format.startsWith("BGR");
    if (!bgr && !format.startsWith("RGB")) throw new Error(`frame format ${format}`);
    const data = new Uint8Array(width * height * 4);
    await frame.copyTo(data, { layout: [{ offset: 0, stride: width * 4 }] });
    const opaque = format.endsWith("X");
    for (let i = 0; i < data.length; i += 4) {
      if (bgr) [data[i], data[i + 2]] = [data[i + 2], data[i]];
      if (opaque) data[i + 3] = 255;
    }
    return data;
  } finally {
    frame.close();
  }
}
export async function readFrame(bitmap) {
  exact ??= createImageBitmap(new Blob([Uint8Array.from(atob(PROBE), (c) => c.charCodeAt(0))]),
    { premultiplyAlpha: "none", colorSpaceConversion: "none" })
    .then((probe) => read(probe).finally(() => probe.close()))
    .then((data) => data.every((v, i) => v === (11 + 37 * i) % 256), () => false);
  return (await exact) ? read(bitmap).catch(() => undefined) : undefined;
}
"#)]
    extern "C" {
        #[wasm_bindgen(js_name = readFrame)]
        pub(super) fn read_frame(bitmap: &web_sys::ImageBitmap) -> js_sys::Promise;
    }
}

/// Reads a bitmap's straight RGBA8 texels through an `OffscreenCanvas` that
/// reads back often: the CPU draws it, without the GPU context and readback
/// a GPU canvas costs.
fn read_canvas(bitmap: &web_sys::ImageBitmap) -> Result<UnassociatedImage, JsValue> {
    let (width, height) = (bitmap.width(), bitmap.height());
    let canvas = web_sys::OffscreenCanvas::new(width, height)?;
    let options = js_sys::Object::new();
    Reflect::set(&options, &"willReadFrequently".into(), &true.into())?;
    let context: web_sys::OffscreenCanvasRenderingContext2d = canvas
        .get_context_with_context_options("2d", &options)?
        .ok_or_else(|| JsValue::from_str("no 2d context"))?
        .dyn_into()?;
    context.draw_image_with_image_bitmap(bitmap, 0.0, 0.0)?;
    let data = context.get_image_data(0.0, 0.0, f64::from(width), f64::from(height))?;
    Ok(UnassociatedImage::from_vec(Size::new(width, height), data.data().0))
}
