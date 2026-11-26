// CSS color strings, read as the style spec's parse_css_color.ts reads them.

use super::{Vector3, display_p3_to_srgb, oklab_via_oklch_to_srgb, oklch_to_srgb};

/// The straight red, green, blue and alpha of a CSS color string: named
/// colors, `transparent`, hex with its `#`, `rgb()`, `rgba()`, `hsl()`,
/// `hsla()`, `oklch()`, `oklab()` and `color(display-p3 ...)`. Red, green and
/// blue are outside 0..1 for a color outside sRGB.
pub(super) fn parse(text: &str) -> Option<(Vector3, f64)> {
    let text = text.trim().to_ascii_lowercase();
    if text == "transparent" {
        return Some(([0.0; 3], 0.0));
    }
    if let Ok(i) = NAMED_COLORS.binary_search_by_key(&text.as_str(), |&(name, _)| name) {
        return Some((NAMED_COLORS[i].1.map(|c| f64::from(c) / 255.0), 1.0));
    }
    if let Some(hex) = text.strip_prefix('#') {
        return parse_hex(hex);
    }
    let (function, arguments) = text.strip_suffix(')')?.split_once('(')?;
    match function {
        "rgb" | "rgba" => parse_rgb(arguments),
        "hsl" | "hsla" => parse_hsl(arguments),
        "oklch" => {
            let [l, c, h, alpha] = css_components(arguments, [1.0, 0.4, 1.0], true)?;
            Some((oklch_to_srgb(l.clamp(0.0, 1.0), c.max(0.0), (h % 360.0).to_radians()), alpha.clamp(0.0, 1.0)))
        }
        "oklab" => {
            let [l, a, b, alpha] = css_components(arguments, [1.0, 0.4, 0.4], false)?;
            Some((oklab_via_oklch_to_srgb(l.clamp(0.0, 1.0), a, b), alpha.clamp(0.0, 1.0)))
        }
        "color" => {
            // color() reads only Display P3.
            let p3 = arguments.trim_start().strip_prefix("display-p3")?;
            if !p3.starts_with(char::is_whitespace) {
                return None;
            }
            let [r, g, b, alpha] = css_components(p3, [1.0; 3], false)?;
            Some((display_p3_to_srgb([r, g, b]), alpha.clamp(0.0, 1.0)))
        }
        _ => None,
    }
}

/// 3, 4, 6 or 8 hex digits: red, green, blue and an optional alpha.
fn parse_hex(hex: &str) -> Option<(Vector3, f64)> {
    let digits = match hex.len() {
        3 | 4 => 1,
        6 | 8 => 2,
        _ => return None,
    };
    if !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |i: usize| {
        let value = u8::from_str_radix(hex.get(i * digits..(i + 1) * digits)?, 16).ok()?;
        // A single digit stands for itself twice: f is ff.
        Some(f64::from(if digits == 1 { value * 17 } else { value }) / 255.0)
    };
    let alpha = if hex.len() == 3 * digits { 1.0 } else { channel(3)? };
    Some(([channel(0)?, channel(1)?, channel(2)?], alpha))
}

// rgb() and hsl() read numbers as the spec does, a run of `[\de.+-]` that
// JS's `+` converts: Rust's parser reads the same runs to the same values,
// and fails on those JS makes NaN of.

/// A value of `rgb()` or `hsl()`, and the unit after it.
fn legacy_value(text: &str) -> Option<(f64, &str)> {
    let end = text.find(|c: char| !(c.is_ascii_digit() || matches!(c, 'e' | '.' | '+' | '-'))).unwrap_or(text.len());
    let (number, unit) = text.split_at(end);
    Some((number.parse().ok()?, unit))
}

/// The alpha of `rgb()` or `hsl()`, a number or a percentage, 1 when it's
/// left out.
fn legacy_alpha(alpha: Option<&str>) -> Option<f64> {
    let Some(alpha) = alpha else { return Some(1.0) };
    let alpha = match legacy_value(alpha)? {
        (alpha, "") => alpha,
        (alpha, "%") => alpha / 100.0,
        _ => return None,
    };
    Some(alpha.clamp(0.0, 1.0))
}

/// The arguments of `rgb()` and `hsl()`: three values and an optional alpha,
/// either all separated by spaces with the alpha after a slash, or all by
/// commas.
fn legacy_arguments(arguments: &str) -> Option<([&str; 3], Option<&str>)> {
    let is_separator = |c: char| c.is_whitespace() || matches!(c, ',' | '/');
    let mut rest = arguments.trim_start();
    let mut values = [""; 4];
    let mut separators = [' '; 3];
    let mut count = 0;
    loop {
        let end = rest.find(is_separator).unwrap_or(rest.len());
        *values.get_mut(count)? = &rest[..end];
        count += 1;
        let after = &rest[end..];
        let trimmed = after.trim_start();
        if trimmed.is_empty() {
            break;
        }
        rest = match trimmed.chars().next() {
            Some(separator @ (',' | '/')) => {
                *separators.get_mut(count - 1)? = separator;
                trimmed[1..].trim_start()
            }
            _ if trimmed.len() < after.len() => trimmed,
            _ => return None,
        };
    }
    let [first, second, third, alpha] = values;
    match (count, separators) {
        (3, [' ', ' ', _] | [',', ',', _]) => Some(([first, second, third], None)),
        (4, [' ', ' ', '/'] | [',', ',', ',']) => Some(([first, second, third], Some(alpha))),
        _ => None,
    }
}

/// `rgb()`'s arguments: red, green and blue all numbers 0..255 or all
/// percentages, clamped.
fn parse_rgb(arguments: &str) -> Option<(Vector3, f64)> {
    let (values, alpha) = legacy_arguments(arguments)?;
    let mut rgb = [0.0; 3];
    let mut units = [""; 3];
    for (i, value) in values.into_iter().enumerate() {
        (rgb[i], units[i]) = legacy_value(value)?;
    }
    let max = match units {
        ["", "", ""] => 255.0,
        ["%", "%", "%"] => 100.0,
        _ => return None,
    };
    Some((rgb.map(|c| (c / max).clamp(0.0, 1.0)), legacy_alpha(alpha)?))
}

/// `hsl()`'s arguments: a hue in degrees, with or without `deg`, and the
/// saturation and lightness as percentages, clamped.
fn parse_hsl(arguments: &str) -> Option<(Vector3, f64)> {
    let ([hue, saturation, lightness], alpha) = legacy_arguments(arguments)?;
    let (hue, "" | "deg") = legacy_value(hue)? else { return None };
    let (saturation, "%") = legacy_value(saturation)? else { return None };
    let (lightness, "%") = legacy_value(lightness)? else { return None };
    // Deviation: an infinite hue is no angle. The spec reads it, as NaN
    // channels.
    if !hue.is_finite() {
        return None;
    }
    Some((hsl_to_srgb(hue, saturation.clamp(0.0, 100.0), lightness.clamp(0.0, 100.0)), legacy_alpha(alpha)?))
}

/// CSS Color 4's HSL to sRGB, as the style spec's `color_spaces.ts` converts
/// it, the saturation and lightness 0..100.
fn hsl_to_srgb(hue: f64, saturation: f64, lightness: f64) -> Vector3 {
    let hue = hue % 360.0;
    let hue = if hue < 0.0 { hue + 360.0 } else { hue };
    let (s, l) = (saturation / 100.0, lightness / 100.0);
    let a = s * l.min(1.0 - l);
    let f = |n: f64| {
        let k = (n + hue / 30.0) % 12.0;
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    [f(0.0), f(8.0), f(4.0)]
}

// CSS Color 4's oklch(), oklab() and color(display-p3 ...): space-separated
// numbers, percentages and `none` (0), an optional alpha after a slash, and
// the hue's angle units.

/// `[+-]?(\d+(\.\d+)?|\.\d+)(e[+-]?\d+)?` at the start of `text`, and what
/// follows it.
fn css_number(text: &str) -> Option<(f64, &str)> {
    let bytes = text.as_bytes();
    let digits =
        |from: usize| bytes.get(from..).map_or(0, |rest| rest.iter().take_while(|c| c.is_ascii_digit()).count());
    let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let whole = digits(end);
    end += whole;
    if bytes.get(end) == Some(&b'.') {
        let fraction = digits(end + 1);
        if fraction == 0 {
            return None;
        }
        end += 1 + fraction;
    } else if whole == 0 {
        return None;
    }
    if bytes.get(end) == Some(&b'e') {
        let sign = usize::from(matches!(bytes.get(end + 1), Some(b'+' | b'-')));
        let exponent = digits(end + 1 + sign);
        if exponent > 0 {
            end += 1 + sign + exponent;
        }
    }
    Some((text[..end].parse().ok()?, &text[end..]))
}

/// A component: `none` is 0, a percentage that share of `percent`, and with
/// `angle` an angle in degrees, its unit's turns wrapped.
fn css_component(text: &str, percent: f64, angle: bool) -> Option<f64> {
    if text == "none" {
        return Some(0.0);
    }
    let (value, unit) = css_number(text)?;
    let degrees_per_unit = match unit {
        "" => return Some(value),
        "%" if !angle => return Some(value / 100.0 * percent),
        "deg" if angle => 1.0,
        "grad" if angle => 0.9,
        "rad" if angle => 180.0 / std::f64::consts::PI,
        "turn" if angle => 360.0,
        _ => return None,
    };
    Some(value % (360.0 / degrees_per_unit) * degrees_per_unit)
}

/// The finite components of a function's arguments `a b c [/ alpha]`, a
/// percentage of each channel a share of `percents`, the third an angle when
/// `angle`. An alpha left out is 1.
fn css_components(arguments: &str, percents: [f64; 3], angle: bool) -> Option<[f64; 4]> {
    let (channels, alpha) = match arguments.split_once('/') {
        Some((channels, alpha)) => (channels, Some(alpha.trim())),
        None => (arguments, None),
    };
    let mut channels = channels.split_whitespace();
    let mut components = [0.0; 4];
    for (i, percent) in percents.into_iter().enumerate() {
        components[i] = css_component(channels.next()?, percent, angle && i == 2)?;
    }
    if channels.next().is_some() {
        return None;
    }
    components[3] = match alpha {
        Some(alpha) if alpha.contains(char::is_whitespace) => return None,
        Some(alpha) => css_component(alpha, 1.0, false)?,
        None => 1.0,
    };
    components.iter().all(|c| c.is_finite()).then_some(components)
}

/// CSS Color 4's named colors (<https://drafts.csswg.org/css-color-4/#named-colors>),
/// sorted by name.
const NAMED_COLORS: [(&str, [u8; 3]); 148] = [
    ("aliceblue", [240, 248, 255]),
    ("antiquewhite", [250, 235, 215]),
    ("aqua", [0, 255, 255]),
    ("aquamarine", [127, 255, 212]),
    ("azure", [240, 255, 255]),
    ("beige", [245, 245, 220]),
    ("bisque", [255, 228, 196]),
    ("black", [0, 0, 0]),
    ("blanchedalmond", [255, 235, 205]),
    ("blue", [0, 0, 255]),
    ("blueviolet", [138, 43, 226]),
    ("brown", [165, 42, 42]),
    ("burlywood", [222, 184, 135]),
    ("cadetblue", [95, 158, 160]),
    ("chartreuse", [127, 255, 0]),
    ("chocolate", [210, 105, 30]),
    ("coral", [255, 127, 80]),
    ("cornflowerblue", [100, 149, 237]),
    ("cornsilk", [255, 248, 220]),
    ("crimson", [220, 20, 60]),
    ("cyan", [0, 255, 255]),
    ("darkblue", [0, 0, 139]),
    ("darkcyan", [0, 139, 139]),
    ("darkgoldenrod", [184, 134, 11]),
    ("darkgray", [169, 169, 169]),
    ("darkgreen", [0, 100, 0]),
    ("darkgrey", [169, 169, 169]),
    ("darkkhaki", [189, 183, 107]),
    ("darkmagenta", [139, 0, 139]),
    ("darkolivegreen", [85, 107, 47]),
    ("darkorange", [255, 140, 0]),
    ("darkorchid", [153, 50, 204]),
    ("darkred", [139, 0, 0]),
    ("darksalmon", [233, 150, 122]),
    ("darkseagreen", [143, 188, 143]),
    ("darkslateblue", [72, 61, 139]),
    ("darkslategray", [47, 79, 79]),
    ("darkslategrey", [47, 79, 79]),
    ("darkturquoise", [0, 206, 209]),
    ("darkviolet", [148, 0, 211]),
    ("deeppink", [255, 20, 147]),
    ("deepskyblue", [0, 191, 255]),
    ("dimgray", [105, 105, 105]),
    ("dimgrey", [105, 105, 105]),
    ("dodgerblue", [30, 144, 255]),
    ("firebrick", [178, 34, 34]),
    ("floralwhite", [255, 250, 240]),
    ("forestgreen", [34, 139, 34]),
    ("fuchsia", [255, 0, 255]),
    ("gainsboro", [220, 220, 220]),
    ("ghostwhite", [248, 248, 255]),
    ("gold", [255, 215, 0]),
    ("goldenrod", [218, 165, 32]),
    ("gray", [128, 128, 128]),
    ("green", [0, 128, 0]),
    ("greenyellow", [173, 255, 47]),
    ("grey", [128, 128, 128]),
    ("honeydew", [240, 255, 240]),
    ("hotpink", [255, 105, 180]),
    ("indianred", [205, 92, 92]),
    ("indigo", [75, 0, 130]),
    ("ivory", [255, 255, 240]),
    ("khaki", [240, 230, 140]),
    ("lavender", [230, 230, 250]),
    ("lavenderblush", [255, 240, 245]),
    ("lawngreen", [124, 252, 0]),
    ("lemonchiffon", [255, 250, 205]),
    ("lightblue", [173, 216, 230]),
    ("lightcoral", [240, 128, 128]),
    ("lightcyan", [224, 255, 255]),
    ("lightgoldenrodyellow", [250, 250, 210]),
    ("lightgray", [211, 211, 211]),
    ("lightgreen", [144, 238, 144]),
    ("lightgrey", [211, 211, 211]),
    ("lightpink", [255, 182, 193]),
    ("lightsalmon", [255, 160, 122]),
    ("lightseagreen", [32, 178, 170]),
    ("lightskyblue", [135, 206, 250]),
    ("lightslategray", [119, 136, 153]),
    ("lightslategrey", [119, 136, 153]),
    ("lightsteelblue", [176, 196, 222]),
    ("lightyellow", [255, 255, 224]),
    ("lime", [0, 255, 0]),
    ("limegreen", [50, 205, 50]),
    ("linen", [250, 240, 230]),
    ("magenta", [255, 0, 255]),
    ("maroon", [128, 0, 0]),
    ("mediumaquamarine", [102, 205, 170]),
    ("mediumblue", [0, 0, 205]),
    ("mediumorchid", [186, 85, 211]),
    ("mediumpurple", [147, 112, 219]),
    ("mediumseagreen", [60, 179, 113]),
    ("mediumslateblue", [123, 104, 238]),
    ("mediumspringgreen", [0, 250, 154]),
    ("mediumturquoise", [72, 209, 204]),
    ("mediumvioletred", [199, 21, 133]),
    ("midnightblue", [25, 25, 112]),
    ("mintcream", [245, 255, 250]),
    ("mistyrose", [255, 228, 225]),
    ("moccasin", [255, 228, 181]),
    ("navajowhite", [255, 222, 173]),
    ("navy", [0, 0, 128]),
    ("oldlace", [253, 245, 230]),
    ("olive", [128, 128, 0]),
    ("olivedrab", [107, 142, 35]),
    ("orange", [255, 165, 0]),
    ("orangered", [255, 69, 0]),
    ("orchid", [218, 112, 214]),
    ("palegoldenrod", [238, 232, 170]),
    ("palegreen", [152, 251, 152]),
    ("paleturquoise", [175, 238, 238]),
    ("palevioletred", [219, 112, 147]),
    ("papayawhip", [255, 239, 213]),
    ("peachpuff", [255, 218, 185]),
    ("peru", [205, 133, 63]),
    ("pink", [255, 192, 203]),
    ("plum", [221, 160, 221]),
    ("powderblue", [176, 224, 230]),
    ("purple", [128, 0, 128]),
    ("rebeccapurple", [102, 51, 153]),
    ("red", [255, 0, 0]),
    ("rosybrown", [188, 143, 143]),
    ("royalblue", [65, 105, 225]),
    ("saddlebrown", [139, 69, 19]),
    ("salmon", [250, 128, 114]),
    ("sandybrown", [244, 164, 96]),
    ("seagreen", [46, 139, 87]),
    ("seashell", [255, 245, 238]),
    ("sienna", [160, 82, 45]),
    ("silver", [192, 192, 192]),
    ("skyblue", [135, 206, 235]),
    ("slateblue", [106, 90, 205]),
    ("slategray", [112, 128, 144]),
    ("slategrey", [112, 128, 144]),
    ("snow", [255, 250, 250]),
    ("springgreen", [0, 255, 127]),
    ("steelblue", [70, 130, 180]),
    ("tan", [210, 180, 140]),
    ("teal", [0, 128, 128]),
    ("thistle", [216, 191, 216]),
    ("tomato", [255, 99, 71]),
    ("turquoise", [64, 224, 208]),
    ("violet", [238, 130, 238]),
    ("wheat", [245, 222, 179]),
    ("white", [255, 255, 255]),
    ("whitesmoke", [245, 245, 245]),
    ("yellow", [255, 255, 0]),
    ("yellowgreen", [154, 205, 50]),
];

#[cfg(test)]
mod tests;
