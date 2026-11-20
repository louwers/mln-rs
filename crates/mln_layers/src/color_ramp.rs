//! The color ramp textures of the layer types that color by a ramp.

use mln_style::StyleValue as _;
use mln_style::color_ramp_property_value::ColorRampPropertyValue;
use mln_style::expression::GlobalStateMap;
use mln_util::image::PremultipliedImage;

/// Fills `image`, a row of pixels, with `color_value`'s ramp from 0 to 1,
/// with `global_state` for its `global-state` expressions. False, leaving
/// the image as it was, where the value is undefined.
pub(crate) fn apply_color_ramp(
    color_value: &ColorRampPropertyValue,
    image: &mut PremultipliedImage,
    global_state: Option<&GlobalStateMap>,
) -> bool {
    if color_value.is_undefined() {
        return false;
    }

    let length = image.bytes();

    let mut i: u32 = 0;
    while (i as usize) < length {
        let color = color_value.evaluate_ramp(f64::from(i) / length as f64, global_state);
        let i_ = i as usize;
        image.data[i_] = (color.r * 255.0).floor() as u8;
        image.data[i_ + 1] = (color.g * 255.0).floor() as u8;
        image.data[i_ + 2] = (color.b * 255.0).floor() as u8;
        image.data[i_ + 3] = (color.a * 255.0).floor() as u8;
        i += 4;
    }
    true
}
