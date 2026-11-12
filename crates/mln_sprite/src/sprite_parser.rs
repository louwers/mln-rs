// Ports src/mln/sprite/sprite_parser.hpp and src/mln/sprite/sprite_parser.cpp.
//
// The JSON is parsed with serde_json (`preserve_order`, so members come in
// document order as with rapidjson). Deviations: syntax
// error messages after "Failed to parse JSON: " are serde_json's, not
// rapidjson's "<message> at offset N"; with duplicate keys rapidjson visits
// every member, serde_json keeps the last value; rapidjson does not
// validate UTF-8, serde_json rejects invalid UTF-8.

use geometry::Point;
use mln_image::decode_image;
use mln_style_types::image::{Image, ImageContent, ImageStretches, Impl, TextFit};
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::immutable::Immutable;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use mln_util::string::to_string_double;
use serde_json::Value as JSValue;

/// `createStyleImage`: extracts an individual image from a spritesheet
/// from the given location. Logs and returns `None` for invalid metrics or
/// metadata.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn create_style_image(
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

/// `value.HasMember(property)` + `value[property]`.
fn member<'a>(value: &'a JSValue, property: &str) -> Option<&'a JSValue> {
    value.as_object().and_then(|object| object.get(property))
}

fn get_u_int16(value: &JSValue, property: &str, name: &str, def: u16) -> u16 {
    if let Some(v) = member(value, property) {
        // rapidjson `IsUint() && GetUint() <= 65535`.
        match v.as_u64() {
            Some(u) if u16::try_from(u).is_ok() => return u as u16,
            _ => Log::warning(
                Event::Sprite,
                format!("Invalid sprite image '{name}': value of '{property}' must be an integer between 0 and 65535"),
            ),
        }
    }

    def
}

fn get_double(value: &JSValue, property: &str, name: &str, def: f64) -> f64 {
    if let Some(v) = member(value, property) {
        match v.as_f64() {
            Some(d) => return d,
            None => Log::warning(
                Event::Sprite,
                format!("Invalid sprite image '{name}': value of '{property}' must be a number"),
            ),
        }
    }

    def
}

fn get_boolean(value: &JSValue, property: &str, name: &str, def: bool) -> bool {
    if let Some(v) = member(value, property) {
        match v.as_bool() {
            Some(b) => return b,
            None => Log::warning(
                Event::Sprite,
                format!("Invalid sprite image '{name}': value of '{property}' must be a boolean"),
            ),
        }
    }

    def
}

fn get_stretches(value: &JSValue, property: &str, name: &str) -> ImageStretches {
    let mut stretches = ImageStretches::new();

    if let Some(v) = member(value, property) {
        if let Some(array) = v.as_array() {
            for stretch in array {
                match stretch.as_array() {
                    Some(pair) if pair.len() == 2 && pair[0].is_number() && pair[1].is_number() => {
                        // GetFloat() is static_cast<float>(GetDouble()).
                        stretches
                            .push((pair[0].as_f64().unwrap_or(0.0) as f32, pair[1].as_f64().unwrap_or(0.0) as f32));
                    }
                    _ => Log::warning(
                        Event::Sprite,
                        format!(
                            "Invalid sprite image '{name}': members of '{property}' must be an array of two numbers"
                        ),
                    ),
                }
            }
        } else {
            Log::warning(
                Event::Sprite,
                format!("Invalid sprite image '{name}': value of '{property}' must be an array"),
            );
        }
    }

    stretches
}

fn get_content(value: &JSValue, property: &str, name: &str) -> Option<ImageContent> {
    if let Some(content) = member(value, property) {
        match content.as_array() {
            Some(c) if c.len() == 4 && c.iter().all(JSValue::is_number) => {
                let f = |i: usize| c[i].as_f64().unwrap_or(0.0) as f32;
                return Some(ImageContent { left: f(0), top: f(1), right: f(2), bottom: f(3) });
            }
            _ => Log::warning(
                Event::Sprite,
                format!("Invalid sprite image '{name}': value of '{property}' must be an array of four numbers"),
            ),
        }
    }

    None
}

fn parse_text_fit(value: &str) -> Option<TextFit> {
    match value {
        "stretchOrShrink" => Some(TextFit::StretchOrShrink),
        "stretchOnly" => Some(TextFit::StretchOnly),
        "proportional" => Some(TextFit::Proportional),
        _ => None,
    }
}

fn get_text_fit(value: &JSValue, property: &str, name: &str) -> Option<TextFit> {
    if let Some(v) = member(value, property) {
        if let Some(value_string) = v.as_str() {
            let text_fit = parse_text_fit(value_string);
            if text_fit.is_none() {
                Log::warning(
                    Event::Sprite,
                    format!(
                        "Invalid sprite image '{name}': value of '{property}' is an invalid value '{value_string}'"
                    ),
                );
            }
            return text_fit;
        }
        Log::warning(Event::Sprite, format!("Invalid sprite image '{name}': value of '{property}' must be a string"));
    }

    None
}

/// `parseSprite(id, image, json)`: parses an image and an associated JSON
/// file and returns the sprite objects. C++ throws `std::runtime_error` (or
/// the image decoder's exception).
pub fn parse_sprite(id: &str, encoded_image: &[u8], json: &[u8]) -> Result<Vec<Immutable<Impl>>, Error> {
    let raster = decode_image(encoded_image)?;
    parse_sprite_images(id, &raster, json)
}

/// Port-only: `parseSprite` after its `decodeImage`, for an image decoded
/// elsewhere (the `SpriteLoader` decodes through `mln_image`'s
/// `decoder`, which may answer later).
pub fn parse_sprite_images(id: &str, raster: &PremultipliedImage, json: &[u8]) -> Result<Vec<Immutable<Impl>>, Error> {
    // `doc.Parse<0>(json.c_str())`: the string ends at the first NUL.
    let json = match json.iter().position(|&b| b == 0) {
        Some(nul) => &json[..nul],
        None => json,
    };
    let doc: JSValue = serde_json::from_slice(json).map_err(|e| Error::native(format!("Failed to parse JSON: {e}")))?;

    let Some(properties) = doc.as_object() else {
        return Err(Error::native("Sprite JSON root must be an object"));
    };

    let mut images = Vec::with_capacity(properties.len());
    for (name, value) in properties {
        let complete_name = if id == "default" { name.clone() } else { format!("{id}:{name}") };

        if value.is_object() {
            let x = get_u_int16(value, "x", name, 0);
            let y = get_u_int16(value, "y", name, 0);
            let width = get_u_int16(value, "width", name, 0);
            let height = get_u_int16(value, "height", name, 0);
            let pixel_ratio = get_double(value, "pixelRatio", name, 1.0);
            let sdf = get_boolean(value, "sdf", name, false);
            let stretch_x = get_stretches(value, "stretchX", name);
            let stretch_y = get_stretches(value, "stretchY", name);
            let content = get_content(value, "content", name);
            let text_fit_width = get_text_fit(value, "textFitWidth", name);
            let text_fit_height = get_text_fit(value, "textFitHeight", name);

            let image = create_style_image(
                &complete_name,
                raster,
                i32::from(x),
                i32::from(y),
                i32::from(width),
                i32::from(height),
                pixel_ratio,
                sdf,
                stretch_x,
                stretch_y,
                content,
                text_fit_width,
                text_fit_height,
            );
            if let Some(image) = image {
                images.push(image.base_impl);
            }
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
