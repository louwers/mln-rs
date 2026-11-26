// The JSON is parsed with serde_json (`preserve_order`, so members come in
// document order as with rapidjson). Deviations: syntax
// error messages after "Failed to parse JSON: " are serde_json's, not
// rapidjson's "<message> at offset N"; with duplicate keys rapidjson visits
// every member, serde_json keeps the last value; rapidjson does not
// validate UTF-8, serde_json rejects invalid UTF-8.

use std::sync::Arc;

use geometry::Point;
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::Error;
use mln_style::image::{Image, ImageContent, ImageStretches, Impl, TextFit};
use mln_style::size::Size;
use mln_style::string::to_string_double;
use serde_json::{Map, Value as JSValue};

use crate::exec::pool::Respond;
use crate::image::decoder;
use crate::log_target;
use crate::storage::Data;

/// Extracts an individual image from a spritesheet at the given location.
/// Logs and returns `None` for invalid metrics or metadata.
#[expect(
    clippy::too_many_arguments,
    reason = "the sprite sheet, the image's rectangle, and the metadata of one sprite entry"
)]
#[must_use]
pub(crate) fn create_style_image(
    id: &str,
    image: &PremultipliedImage,
    src_x: i32,
    src_y: i32,
    width: i32,
    height: i32,
    ratio: f64,
    sdf: bool,
    stretch_x: ImageStretches,
    stretch_y: ImageStretches,
    content: Option<ImageContent>,
    text_fit_width: Option<TextFit>,
    text_fit_height: Option<TextFit>,
) -> Option<Image> {
    // Disallow invalid parameter configurations.
    let image_width = image.size.width as i32;
    let image_height = image.size.height as i32;
    if width <= 0
        || height <= 0
        || width > 1024
        || height > 1024
        || ratio <= 0.0
        || ratio > 10.0
        || src_x < 0
        || src_y < 0
        || i64::from(src_x) >= i64::from(image.size.width)
        || i64::from(src_y) >= i64::from(image.size.height)
        || src_x.wrapping_add(width) > image_width
        || src_y.wrapping_add(height) > image_height
    {
        let message = format!(
            "Can't create image with invalid metrics: {}x{}@{},{} in {}x{}@{}x sprite",
            width,
            height,
            src_x,
            src_y,
            image.size.width,
            image.size.height,
            to_string_double(ratio)
        );
        log::error!(target: log_target::SPRITE, "{message}");
        return None;
    }

    let size = Size::new(width as u32, height as u32);
    let mut dst_image = PremultipliedImage::new(size);

    // Copy from the source image into our individual sprite image
    PremultipliedImage::copy(image, &mut dst_image, Point::new(src_x as u32, src_y as u32), Point::new(0, 0), size)
        .expect("sprite metrics were validated");

    match Image::new(
        id.to_owned(),
        dst_image,
        ratio as f32,
        sdf,
        stretch_x,
        stretch_y,
        content,
        text_fit_width,
        text_fit_height,
    ) {
        Ok(image) => Some(image),
        Err(ex) => {
            log::error!(target: log_target::SPRITE, "Can't create image with invalid metadata: {ex}");
            None
        }
    }
}

fn warn(name: &str, message: std::fmt::Arguments<'_>) {
    log::warn!(target: log_target::SPRITE, "Invalid sprite image '{name}': {message}");
}

/// The member `property` of the sprite entry `name`, as `read` takes it. A
/// member that `read` rejects is logged as one that `must` hold something
/// else, and treated as absent.
fn member<'a, T>(
    entry: &'a Map<String, JSValue>,
    name: &str,
    property: &str,
    must: &str,
    read: impl FnOnce(&'a JSValue) -> Option<T>,
) -> Option<T> {
    let value = read(entry.get(property)?);
    if value.is_none() {
        warn(name, format_args!("value of '{property}' must be {must}"));
    }
    value
}

fn integer(entry: &Map<String, JSValue>, name: &str, property: &str) -> u16 {
    member(entry, name, property, "an integer between 0 and 65535", |value| {
        value.as_u64().and_then(|value| u16::try_from(value).ok())
    })
    .unwrap_or(0)
}

fn stretches(entry: &Map<String, JSValue>, name: &str, property: &str) -> ImageStretches {
    let Some(pairs) = member(entry, name, property, "an array", JSValue::as_array) else {
        return ImageStretches::new();
    };
    pairs
        .iter()
        .filter_map(|pair| {
            let stretch = match pair.as_array().map(Vec::as_slice) {
                Some([start, end]) => start.as_f64().zip(end.as_f64()),
                _ => None,
            };
            if stretch.is_none() {
                warn(name, format_args!("members of '{property}' must be an array of two numbers"));
            }
            stretch.map(|(start, end)| (start as f32, end as f32))
        })
        .collect()
}

fn content(entry: &Map<String, JSValue>, name: &str, property: &str) -> Option<ImageContent> {
    member(entry, name, property, "an array of four numbers", |value| {
        let [left, top, right, bottom] = value.as_array()?.as_slice() else { return None };
        let side = |value: &JSValue| value.as_f64().map(|value| value as f32);
        Some(ImageContent { left: side(left)?, top: side(top)?, right: side(right)?, bottom: side(bottom)? })
    })
}

fn text_fit(entry: &Map<String, JSValue>, name: &str, property: &str) -> Option<TextFit> {
    let value = member(entry, name, property, "a string", JSValue::as_str)?;
    let text_fit = match value {
        "stretchOrShrink" => Some(TextFit::StretchOrShrink),
        "stretchOnly" => Some(TextFit::StretchOnly),
        "proportional" => Some(TextFit::Proportional),
        _ => None,
    };
    if text_fit.is_none() {
        warn(name, format_args!("value of '{property}' is an invalid value '{value}'"));
    }
    text_fit
}

/// Decodes `image` and cuts it into the images that `json`, a sprite index,
/// lists, as [`parse_sprite`] does. Answers through `respond`, at once or,
/// where the platform decodes images asynchronously (the browser), later.
#[expect(clippy::needless_pass_by_value, reason = "a job owns its inputs, as one sent to another thread does")]
pub fn load_sprite(id: String, image: Data, json: Data, respond: Respond<Result<Vec<Arc<Impl>>, Error>>) {
    decoder::decode(&image, move |raster| respond(raster.and_then(|raster| parse_sprite(&id, &raster, &json))));
}

/// Cuts `image`, a decoded sprite sheet, into the images that `json`, a
/// sprite index, lists. An image named `name` is `name` in the sprite
/// `default` and `id:name` in another. Fails when `json` isn't JSON or isn't
/// an object. An entry that isn't an object is skipped, a member of the
/// wrong type is logged and treated as absent, and an image whose metrics
/// don't fit the sheet is logged and skipped.
pub fn parse_sprite(id: &str, image: &PremultipliedImage, json: &[u8]) -> Result<Vec<Arc<Impl>>, Error> {
    // `doc.Parse<0>(json.c_str())`: the string ends at the first NUL.
    let json = match json.iter().position(|&b| b == 0) {
        Some(nul) => &json[..nul],
        None => json,
    };
    let doc: JSValue = serde_json::from_slice(json).map_err(|e| Error::native(format!("Failed to parse JSON: {e}")))?;
    let JSValue::Object(entries) = doc else {
        return Err(Error::native("Sprite JSON root must be an object"));
    };

    let mut images = Vec::with_capacity(entries.len());
    for (name, value) in &entries {
        let Some(entry) = value.as_object() else { continue };
        let complete_name = if id == "default" { name.clone() } else { format!("{id}:{name}") };
        // The members are read, and their warnings logged, in this order.
        let image = create_style_image(
            &complete_name,
            image,
            i32::from(integer(entry, name, "x")),
            i32::from(integer(entry, name, "y")),
            i32::from(integer(entry, name, "width")),
            i32::from(integer(entry, name, "height")),
            member(entry, name, "pixelRatio", "a number", JSValue::as_f64).unwrap_or(1.0),
            member(entry, name, "sdf", "a boolean", JSValue::as_bool).unwrap_or(false),
            stretches(entry, name, "stretchX"),
            stretches(entry, name, "stretchY"),
            content(entry, name, "content"),
            text_fit(entry, name, "textFitWidth"),
            text_fit(entry, name, "textFitHeight"),
        );
        if let Some(image) = image {
            images.push(image.base_impl);
        }
    }

    // C++: assert([&images] { std::sort(images.begin(), images.end());
    // return std::unique(images.begin(), images.end()) == images.end(); }());
    // The assertion sorts `images` (by id) as a side effect, so builds with
    // assertions return them sorted and NDEBUG builds in document order.
    // Ported as is: debug_assertions plays the role of !NDEBUG.
    if cfg!(debug_assertions) {
        images.sort_by(|a, b| a.id.cmp(&b.id));
        assert!(images.windows(2).all(|w| w[0].id != w[1].id), "duplicate sprite image ids");
    }
    Ok(images)
}
