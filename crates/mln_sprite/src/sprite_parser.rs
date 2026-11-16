// The JSON is parsed with serde_json (`preserve_order`, so members come in
// document order as with rapidjson). Deviations: syntax
// error messages after "Failed to parse JSON: " are serde_json's, not
// rapidjson's "<message> at offset N"; with duplicate keys rapidjson visits
// every member, serde_json keeps the last value; rapidjson does not
// validate UTF-8, serde_json rejects invalid UTF-8.

use std::sync::Arc;

use geometry::Point;
use mln_actor::scheduler::Respond;
use mln_image::decoder;
use mln_storage::Data;
use mln_style_types::image::{Image, ImageContent, ImageStretches, Impl, TextFit};
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use mln_util::string::to_string_double;
use serde::Deserialize;
use serde::de::Deserializer;
use serde_json::Value as JSValue;

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
        Log::error(Event::Sprite, message);
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
            Log::error(Event::Sprite, format!("Can't create image with invalid metadata: {ex}"));
            None
        }
    }
}

/// A member of a sprite entry: absent, of the type it should be, or of
/// another type, which the entry warns about and treats as absent.
///
/// It deserializes from a `serde_json::Value` member, which a type error
/// leaves as it was, so the error can stand for the member.
#[derive(Default)]
enum Lenient<T> {
    #[default]
    Absent,
    Valid(T),
    Invalid,
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Lenient<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(T::deserialize(deserializer).map_or(Lenient::Invalid, Lenient::Valid))
    }
}

/// An image of a sprite index. serde's conversions check what the members
/// hold: a `u16` takes an integer from 0 to 65535, an `f64` any number, and a
/// pair or a quad exactly that many numbers.
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SpriteEntry {
    x: Lenient<u16>,
    y: Lenient<u16>,
    width: Lenient<u16>,
    height: Lenient<u16>,
    pixel_ratio: Lenient<f64>,
    sdf: Lenient<bool>,
    stretch_x: Lenient<Vec<Lenient<(f64, f64)>>>,
    stretch_y: Lenient<Vec<Lenient<(f64, f64)>>>,
    content: Lenient<[f64; 4]>,
    text_fit_width: Lenient<String>,
    text_fit_height: Lenient<String>,
}

/// What a sprite entry describes, with the defaults for what it lacks or
/// gets wrong.
struct Metrics {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    pixel_ratio: f64,
    sdf: bool,
    stretch_x: ImageStretches,
    stretch_y: ImageStretches,
    content: Option<ImageContent>,
    text_fit_width: Option<TextFit>,
    text_fit_height: Option<TextFit>,
}

fn warn(name: &str, message: std::fmt::Arguments<'_>) {
    Log::warning(Event::Sprite, format!("Invalid sprite image '{name}': {message}"));
}

/// The value of a member, or `default` with the warning `invalid` gives
/// when it has another type.
fn value_or<T>(member: Lenient<T>, default: T, invalid: impl FnOnce()) -> T {
    match member {
        Lenient::Absent => default,
        Lenient::Valid(value) => value,
        Lenient::Invalid => {
            invalid();
            default
        }
    }
}

fn integer(member: Lenient<u16>, name: &str, property: &str) -> u16 {
    value_or(member, 0, || warn(name, format_args!("value of '{property}' must be an integer between 0 and 65535")))
}

fn number(member: Lenient<f64>, name: &str, property: &str, default: f64) -> f64 {
    value_or(member, default, || warn(name, format_args!("value of '{property}' must be a number")))
}

fn boolean(member: Lenient<bool>, name: &str, property: &str) -> bool {
    value_or(member, false, || warn(name, format_args!("value of '{property}' must be a boolean")))
}

fn stretches(member: Lenient<Vec<Lenient<(f64, f64)>>>, name: &str, property: &str) -> ImageStretches {
    let pairs = value_or(member, Vec::new(), || warn(name, format_args!("value of '{property}' must be an array")));
    pairs
        .into_iter()
        .filter_map(|pair| match pair {
            Lenient::Valid((start, end)) => Some((start as f32, end as f32)),
            Lenient::Absent | Lenient::Invalid => {
                warn(name, format_args!("members of '{property}' must be an array of two numbers"));
                None
            }
        })
        .collect()
}

fn content(member: Lenient<[f64; 4]>, name: &str, property: &str) -> Option<ImageContent> {
    let content = value_or(member.map(Some), None, || {
        warn(name, format_args!("value of '{property}' must be an array of four numbers"));
    })?;
    let [left, top, right, bottom] = content.map(|value| value as f32);
    Some(ImageContent { left, top, right, bottom })
}

fn text_fit(member: Lenient<String>, name: &str, property: &str) -> Option<TextFit> {
    let value =
        value_or(member.map(Some), None, || warn(name, format_args!("value of '{property}' must be a string")))?;
    let text_fit = match value.as_str() {
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

impl<T> Lenient<T> {
    fn map<U>(self, f: impl FnOnce(T) -> U) -> Lenient<U> {
        match self {
            Lenient::Absent => Lenient::Absent,
            Lenient::Valid(value) => Lenient::Valid(f(value)),
            Lenient::Invalid => Lenient::Invalid,
        }
    }
}

impl SpriteEntry {
    /// The entry's metrics, warning about each member of the wrong type, in
    /// the order of the fields.
    fn metrics(self, name: &str) -> Metrics {
        Metrics {
            x: integer(self.x, name, "x"),
            y: integer(self.y, name, "y"),
            width: integer(self.width, name, "width"),
            height: integer(self.height, name, "height"),
            pixel_ratio: number(self.pixel_ratio, name, "pixelRatio", 1.0),
            sdf: boolean(self.sdf, name, "sdf"),
            stretch_x: stretches(self.stretch_x, name, "stretchX"),
            stretch_y: stretches(self.stretch_y, name, "stretchY"),
            content: content(self.content, name, "content"),
            text_fit_width: text_fit(self.text_fit_width, name, "textFitWidth"),
            text_fit_height: text_fit(self.text_fit_height, name, "textFitHeight"),
        }
    }
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
        let Some(members) = value.as_object() else { continue };
        // An object always deserializes: each member is `Lenient`.
        let Ok(entry) = SpriteEntry::deserialize(members) else { continue };
        let metrics = entry.metrics(name);
        let complete_name = if id == "default" { name.clone() } else { format!("{id}:{name}") };
        let image = create_style_image(
            &complete_name,
            image,
            i32::from(metrics.x),
            i32::from(metrics.y),
            i32::from(metrics.width),
            i32::from(metrics.height),
            metrics.pixel_ratio,
            metrics.sdf,
            metrics.stretch_x,
            metrics.stretch_y,
            metrics.content,
            metrics.text_fit_width,
            metrics.text_fit_height,
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
