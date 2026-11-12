//! Ports vendor/csscolorparser (csscolorparser.cpp, csscolorparser.hpp):
//! (c) Dean McNamee <dean@gmail.com>, 2012; C++ port by Mapbox, Konstantin
//! Käfer <mail@kkaefer.com>, 2014-2017; MIT License
//! (<https://github.com/deanm/css-color-parser-js>,
//! <https://github.com/kkaefer/css-color-parser-cpp>).
//!
//! The C++ works on the bytes of a `std::string` and leans on C library
//! behaviour (`strtof`, `strtoll`, `std::getline`, `std::modf`, x86 float to
//! integer conversion); all of it is reproduced here. `cstdlib` exposes the
//! glibc `strtof`/`strtod`/`strtoll` ports for other users.

mod bigint;
pub mod cstdlib;

use cstdlib::{strtof, strtoll};

/// `CSSColorParser::Color`.
#[derive(Clone, Copy, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

impl Default for Color {
    fn default() -> Self {
        Color { r: 0, g: 0, b: 0, a: 1.0 }
    }
}

impl Color {
    /// The C++ constructor: clamps alpha to 0..1 (NaN passes through).
    #[must_use]
    pub fn new(r: u8, g: u8, b: u8, a: f32) -> Self {
        Color { r, g, b, a: a.clamp(0.0, 1.0) }
    }
}

/// The C++ `operator==`: alpha compares with a tolerance.
impl PartialEq for Color {
    fn eq(&self, rhs: &Color) -> bool {
        // ::fabs(lhs.a - rhs.a) < 0.0001f: the float difference, promoted.
        self.r == rhs.r && self.g == rhs.g && self.b == rhs.b && f64::from(self.a - rhs.a).abs() < f64::from(0.0001f32)
    }
}

// http://www.w3.org/TR/css3-color/
const NAMED_COLORS: [(&str, [u8; 3], f32); 148] = [
    ("transparent", [0, 0, 0], 0.0),
    ("aliceblue", [240, 248, 255], 1.0),
    ("antiquewhite", [250, 235, 215], 1.0),
    ("aqua", [0, 255, 255], 1.0),
    ("aquamarine", [127, 255, 212], 1.0),
    ("azure", [240, 255, 255], 1.0),
    ("beige", [245, 245, 220], 1.0),
    ("bisque", [255, 228, 196], 1.0),
    ("black", [0, 0, 0], 1.0),
    ("blanchedalmond", [255, 235, 205], 1.0),
    ("blue", [0, 0, 255], 1.0),
    ("blueviolet", [138, 43, 226], 1.0),
    ("brown", [165, 42, 42], 1.0),
    ("burlywood", [222, 184, 135], 1.0),
    ("cadetblue", [95, 158, 160], 1.0),
    ("chartreuse", [127, 255, 0], 1.0),
    ("chocolate", [210, 105, 30], 1.0),
    ("coral", [255, 127, 80], 1.0),
    ("cornflowerblue", [100, 149, 237], 1.0),
    ("cornsilk", [255, 248, 220], 1.0),
    ("crimson", [220, 20, 60], 1.0),
    ("cyan", [0, 255, 255], 1.0),
    ("darkblue", [0, 0, 139], 1.0),
    ("darkcyan", [0, 139, 139], 1.0),
    ("darkgoldenrod", [184, 134, 11], 1.0),
    ("darkgray", [169, 169, 169], 1.0),
    ("darkgreen", [0, 100, 0], 1.0),
    ("darkgrey", [169, 169, 169], 1.0),
    ("darkkhaki", [189, 183, 107], 1.0),
    ("darkmagenta", [139, 0, 139], 1.0),
    ("darkolivegreen", [85, 107, 47], 1.0),
    ("darkorange", [255, 140, 0], 1.0),
    ("darkorchid", [153, 50, 204], 1.0),
    ("darkred", [139, 0, 0], 1.0),
    ("darksalmon", [233, 150, 122], 1.0),
    ("darkseagreen", [143, 188, 143], 1.0),
    ("darkslateblue", [72, 61, 139], 1.0),
    ("darkslategray", [47, 79, 79], 1.0),
    ("darkslategrey", [47, 79, 79], 1.0),
    ("darkturquoise", [0, 206, 209], 1.0),
    ("darkviolet", [148, 0, 211], 1.0),
    ("deeppink", [255, 20, 147], 1.0),
    ("deepskyblue", [0, 191, 255], 1.0),
    ("dimgray", [105, 105, 105], 1.0),
    ("dimgrey", [105, 105, 105], 1.0),
    ("dodgerblue", [30, 144, 255], 1.0),
    ("firebrick", [178, 34, 34], 1.0),
    ("floralwhite", [255, 250, 240], 1.0),
    ("forestgreen", [34, 139, 34], 1.0),
    ("fuchsia", [255, 0, 255], 1.0),
    ("gainsboro", [220, 220, 220], 1.0),
    ("ghostwhite", [248, 248, 255], 1.0),
    ("gold", [255, 215, 0], 1.0),
    ("goldenrod", [218, 165, 32], 1.0),
    ("gray", [128, 128, 128], 1.0),
    ("green", [0, 128, 0], 1.0),
    ("greenyellow", [173, 255, 47], 1.0),
    ("grey", [128, 128, 128], 1.0),
    ("honeydew", [240, 255, 240], 1.0),
    ("hotpink", [255, 105, 180], 1.0),
    ("indianred", [205, 92, 92], 1.0),
    ("indigo", [75, 0, 130], 1.0),
    ("ivory", [255, 255, 240], 1.0),
    ("khaki", [240, 230, 140], 1.0),
    ("lavender", [230, 230, 250], 1.0),
    ("lavenderblush", [255, 240, 245], 1.0),
    ("lawngreen", [124, 252, 0], 1.0),
    ("lemonchiffon", [255, 250, 205], 1.0),
    ("lightblue", [173, 216, 230], 1.0),
    ("lightcoral", [240, 128, 128], 1.0),
    ("lightcyan", [224, 255, 255], 1.0),
    ("lightgoldenrodyellow", [250, 250, 210], 1.0),
    ("lightgray", [211, 211, 211], 1.0),
    ("lightgreen", [144, 238, 144], 1.0),
    ("lightgrey", [211, 211, 211], 1.0),
    ("lightpink", [255, 182, 193], 1.0),
    ("lightsalmon", [255, 160, 122], 1.0),
    ("lightseagreen", [32, 178, 170], 1.0),
    ("lightskyblue", [135, 206, 250], 1.0),
    ("lightslategray", [119, 136, 153], 1.0),
    ("lightslategrey", [119, 136, 153], 1.0),
    ("lightsteelblue", [176, 196, 222], 1.0),
    ("lightyellow", [255, 255, 224], 1.0),
    ("lime", [0, 255, 0], 1.0),
    ("limegreen", [50, 205, 50], 1.0),
    ("linen", [250, 240, 230], 1.0),
    ("magenta", [255, 0, 255], 1.0),
    ("maroon", [128, 0, 0], 1.0),
    ("mediumaquamarine", [102, 205, 170], 1.0),
    ("mediumblue", [0, 0, 205], 1.0),
    ("mediumorchid", [186, 85, 211], 1.0),
    ("mediumpurple", [147, 112, 219], 1.0),
    ("mediumseagreen", [60, 179, 113], 1.0),
    ("mediumslateblue", [123, 104, 238], 1.0),
    ("mediumspringgreen", [0, 250, 154], 1.0),
    ("mediumturquoise", [72, 209, 204], 1.0),
    ("mediumvioletred", [199, 21, 133], 1.0),
    ("midnightblue", [25, 25, 112], 1.0),
    ("mintcream", [245, 255, 250], 1.0),
    ("mistyrose", [255, 228, 225], 1.0),
    ("moccasin", [255, 228, 181], 1.0),
    ("navajowhite", [255, 222, 173], 1.0),
    ("navy", [0, 0, 128], 1.0),
    ("oldlace", [253, 245, 230], 1.0),
    ("olive", [128, 128, 0], 1.0),
    ("olivedrab", [107, 142, 35], 1.0),
    ("orange", [255, 165, 0], 1.0),
    ("orangered", [255, 69, 0], 1.0),
    ("orchid", [218, 112, 214], 1.0),
    ("palegoldenrod", [238, 232, 170], 1.0),
    ("palegreen", [152, 251, 152], 1.0),
    ("paleturquoise", [175, 238, 238], 1.0),
    ("palevioletred", [219, 112, 147], 1.0),
    ("papayawhip", [255, 239, 213], 1.0),
    ("peachpuff", [255, 218, 185], 1.0),
    ("peru", [205, 133, 63], 1.0),
    ("pink", [255, 192, 203], 1.0),
    ("plum", [221, 160, 221], 1.0),
    ("powderblue", [176, 224, 230], 1.0),
    ("purple", [128, 0, 128], 1.0),
    ("red", [255, 0, 0], 1.0),
    ("rosybrown", [188, 143, 143], 1.0),
    ("royalblue", [65, 105, 225], 1.0),
    ("saddlebrown", [139, 69, 19], 1.0),
    ("salmon", [250, 128, 114], 1.0),
    ("sandybrown", [244, 164, 96], 1.0),
    ("seagreen", [46, 139, 87], 1.0),
    ("seashell", [255, 245, 238], 1.0),
    ("sienna", [160, 82, 45], 1.0),
    ("silver", [192, 192, 192], 1.0),
    ("skyblue", [135, 206, 235], 1.0),
    ("slateblue", [106, 90, 205], 1.0),
    ("slategray", [112, 128, 144], 1.0),
    ("slategrey", [112, 128, 144], 1.0),
    ("snow", [255, 250, 250], 1.0),
    ("springgreen", [0, 255, 127], 1.0),
    ("steelblue", [70, 130, 180], 1.0),
    ("tan", [210, 180, 140], 1.0),
    ("teal", [0, 128, 128], 1.0),
    ("thistle", [216, 191, 216], 1.0),
    ("tomato", [255, 99, 71], 1.0),
    ("turquoise", [64, 224, 208], 1.0),
    ("violet", [238, 130, 238], 1.0),
    ("wheat", [245, 222, 179], 1.0),
    ("white", [255, 255, 255], 1.0),
    ("whitesmoke", [245, 245, 245], 1.0),
    ("yellow", [255, 255, 0], 1.0),
    ("yellowgreen", [154, 205, 50], 1.0),
];

/// x86-64 `cvttsd2si`: what a double -> `int64_t` conversion compiles to; out
/// of range (UB in C++) gives `INT64_MIN`.
fn double_to_int64(d: f64) -> i64 {
    if d.is_nan() || !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&d) { i64::MIN } else { d as i64 }
}

/// `clamp_css_byte<float>`: clamp to integer 0 .. 255.
fn clamp_css_byte_f32(i: f32) -> u8 {
    let i = i.round(); // Seems to be what Chrome does (vs truncation).
    // uint8_t(NaN) is UB; x86 truncates through int32 (0x80000000), giving 0.
    if i < 0.0 {
        0
    } else if i > 255.0 {
        255
    } else {
        i as u8
    }
}

/// `clamp_css_byte<int64_t>`: `::round(i)` goes through double and back.
fn clamp_css_byte_i64(i: i64) -> u8 {
    let i = double_to_int64((i as f64).round());
    if i < 0 {
        0
    } else if i > 255 {
        255
    } else {
        i as u8
    }
}

/// `clamp_css_float<float>`: clamp to float 0.0 .. 1.0 (NaN passes through).
fn clamp_css_float(f: f32) -> f32 {
    f.clamp(0.0, 1.0)
}

fn parse_float(s: &[u8]) -> f32 {
    strtof(s).0
}

fn parse_int(s: &[u8], base: u32) -> i64 {
    strtoll(s, base).0
}

/// int or percentage.
fn parse_css_int(s: &[u8]) -> u8 {
    if s.last() == Some(&b'%') {
        clamp_css_byte_f32(parse_float(s) / 100.0f32 * 255.0f32)
    } else {
        clamp_css_byte_i64(parse_int(s, 10))
    }
}

/// float or percentage.
fn parse_css_float(s: &[u8]) -> f32 {
    if s.last() == Some(&b'%') { clamp_css_float(parse_float(s) / 100.0f32) } else { clamp_css_float(parse_float(s)) }
}

fn css_hue_to_rgb(m1: f32, m2: f32, mut h: f32) -> f32 {
    if h < 0.0f32 {
        h += 1.0f32;
    } else if h > 1.0f32 {
        h -= 1.0f32;
    }

    if h * 6.0f32 < 1.0f32 {
        return m1 + (m2 - m1) * h * 6.0f32;
    }
    if h * 2.0f32 < 1.0f32 {
        return m2;
    }
    if h * 3.0f32 < 2.0f32 {
        return m1 + (m2 - m1) * (2.0f32 / 3.0f32 - h) * 6.0f32;
    }
    m1
}

/// `std::getline(ss, item, delim)` in a loop: a trailing empty field (after a
/// final delimiter, or of an empty string) is not produced.
fn split(s: &[u8], delim: u8) -> Vec<&[u8]> {
    let mut elems: Vec<&[u8]> = s.split(|&c| c == delim).collect();
    if elems.last().is_some_and(|e| e.is_empty()) {
        elems.pop();
    }
    elems
}

/// `std::modf(float, float*)`: the fractional part, with the sign of the
/// argument (±0 for integers and infinities).
fn modf(x: f32) -> f32 {
    if x.is_infinite() {
        0.0f32.copysign(x)
    } else if x.is_nan() {
        x
    } else {
        (x - x.trunc()).copysign(x)
    }
}

#[must_use]
pub fn parse(css_str: &str) -> Option<Color> {
    // Remove all whitespace, not compliant, but should just be more accepting.
    // Convert to lowercase (::tolower in the C locale only maps A-Z).
    let str: Vec<u8> = css_str.bytes().filter(|&c| c != b' ').map(|c| c.to_ascii_lowercase()).collect();

    for (name, [r, g, b], a) in &NAMED_COLORS {
        if str == name.as_bytes() {
            return Some(Color::new(*r, *g, *b, *a));
        }
    }

    // #abc, #abcd, #abc123, and #abc12345 syntax.
    if str.first() == Some(&b'#') {
        if str.len() == 4 {
            // #RGB
            let iv = parse_int(&str[1..], 16); // TODO(deanm): Stricter parsing.
            if !(0..=0xfff).contains(&iv) {
                return None;
            }
            return Some(Color::new(
                (((iv & 0xf00) >> 4) | ((iv & 0xf00) >> 8)) as u8,
                ((iv & 0xf0) | ((iv & 0xf0) >> 4)) as u8,
                ((iv & 0xf) | ((iv & 0xf) << 4)) as u8,
                1.0,
            ));
        } else if str.len() == 5 {
            // #RGBA
            let iv = parse_int(&str[1..], 16); // TODO(deanm): Stricter parsing.
            if !(0..=0xffff).contains(&iv) {
                return None;
            }
            return Some(Color::new(
                (((iv & 0xf000) >> 8) | ((iv & 0xf000) >> 12)) as u8,
                (((iv & 0xf00) >> 4) | ((iv & 0xf00) >> 8)) as u8,
                ((iv & 0xf0) | ((iv & 0xf0) >> 4)) as u8,
                (((iv & 0xf) | ((iv & 0xf) << 4)) as f32) / 255.0f32,
            ));
        } else if str.len() == 7 {
            // #RRGGBB
            let iv = parse_int(&str[1..], 16); // TODO(deanm): Stricter parsing.
            if !(0..=0x00ff_ffff).contains(&iv) {
                return None; // Covers NaN.
            }
            return Some(Color::new(
                ((iv & 0x00ff_0000) >> 16) as u8,
                ((iv & 0xff00) >> 8) as u8,
                (iv & 0xff) as u8,
                1.0,
            ));
        } else if str.len() == 9 {
            // #RRGGBBAA
            let iv = parse_int(&str[1..], 16); // TODO(deanm): Stricter parsing.
            if !(0..=0xffff_ffff).contains(&iv) {
                return None; // Covers NaN.
            }
            return Some(Color::new(
                ((iv & 0xff00_0000) >> 24) as u8,
                ((iv & 0x00ff_0000) >> 16) as u8,
                ((iv & 0xff00) >> 8) as u8,
                ((iv & 0xff) as f32) / 255.0f32,
            ));
        }

        return None;
    }

    let op = str.iter().position(|&c| c == b'(');
    let ep = str.iter().position(|&c| c == b')');
    // C++: `op != npos && ep + 1 == str.length()` (npos + 1 wraps to 0, which
    // never equals the length of a string that contains '(').
    if let (Some(op), Some(ep)) = (op, ep)
        && ep + 1 == str.len()
    {
        let fname = &str[..op];
        let params = split(&str[op + 1..ep], b',');

        let mut alpha = 1.0f32;

        if fname == b"rgba" || fname == b"rgb" {
            if fname == b"rgba" {
                if params.len() != 4 {
                    return None;
                }
                alpha = parse_css_float(params.last().unwrap());
            } else if params.len() != 3 {
                return None;
            }

            return Some(Color::new(
                parse_css_int(params[0]),
                parse_css_int(params[1]),
                parse_css_int(params[2]),
                alpha,
            ));
        } else if fname == b"hsla" || fname == b"hsl" {
            if fname == b"hsla" {
                if params.len() != 4 {
                    return None;
                }
                alpha = parse_css_float(params.last().unwrap());
            } else if params.len() == 4 {
                alpha = parse_css_float(params.last().unwrap());
            } else if params.len() != 3 {
                return None;
            }

            let mut h = parse_float(params[0]) / 360.0f32;
            // Normalize the hue to [0..1[
            h = modf(h);

            // NOTE(deanm): According to the CSS spec s/l should only be
            // percentages, but we don't bother and let float or percentage.
            let s = parse_css_float(params[1]);
            let l = parse_css_float(params[2]);

            let m2 = if l <= 0.5f32 { l * (s + 1.0f32) } else { l + s - l * s };
            let m1 = l * 2.0f32 - m2;

            return Some(Color::new(
                clamp_css_byte_f32(css_hue_to_rgb(m1, m2, h + 1.0f32 / 3.0f32) * 255.0f32),
                clamp_css_byte_f32(css_hue_to_rgb(m1, m2, h) * 255.0f32),
                clamp_css_byte_f32(css_hue_to_rgb(m1, m2, h - 1.0f32 / 3.0f32) * 255.0f32),
                alpha,
            ));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    // Smoke tests; //difftest/util checks the port against the C++.
    use super::*;

    fn c(r: u8, g: u8, b: u8, a: f32) -> Color {
        Color::new(r, g, b, a)
    }

    #[test]
    fn upstream() {
        assert_eq!(Some(c(255, 128, 12, 0.5)), parse(" rgba (255, 128, 12, 0.5)"));
        assert_eq!(Some(c(255, 255, 255, 1.0)), parse("#fff"));
        assert_eq!(Some(c(255, 0, 17, 1.0)), parse("#ff0011"));
        assert_eq!(Some(c(106, 90, 205, 1.0)), parse("slateblue"));
        assert_eq!(None, parse("blah"));
        assert_eq!(None, parse("ffffff"));
        assert_eq!(Some(c(226, 233, 233, 0.5)), parse("hsla(900, 15%, 90%, 0.5)"));
        assert_eq!(None, parse("hsla(900, 15%, 90%)"));
        assert_eq!(Some(c(226, 233, 233, 1.0)), parse("hsl(900, 15%, 90%)"));
        assert_eq!(Some(c(226, 233, 233, 1.0)), parse("hsl(900, 0.15, 90%)")); // NOTE: not spec compliant.
        assert_eq!(Some(c(0, 0, 0, 1.0)), parse("hsl(9999999999999999999, 0, 0)")); // NOTE: float precision loss
        assert_eq!(Some(c(255, 191, 0, 1.0)), parse("hsl(45, 100%, 50%)"));
        assert_eq!(Some(c(255, 191, 0, 1.0)), parse("hsl(-315, 100%, 50%)"));
        assert_eq!(Some(c(255, 191, 0, 1.0)), parse("hsl(-675, 100%, 50%)"));
        // Out of range:
        assert_eq!(None, parse("xxx"));
        assert_eq!(Some(c(255, 128, 12, 1.0)), parse(" rgba (255, 128, 12, 2)"));
        assert_eq!(Some(c(255, 128, 12, 1.0)), parse(" rgba (400, 128, 12, 2)"));
        // 5% -> 0.05f * 255 = 12.75 -> 13
        assert_eq!(Some(c(255, 128, 13, 1.0)), parse(" rgba (100%, 50%, 5%, 2)"));
        assert_eq!(Some(c(255, 128, 13, 0.0)), parse(" rgba (100%, 50%, 5%, -2)"));
        assert_eq!(Some(c(255, 128, 13, 1.0)), parse(" rgba (100%, 50%, 5%, 1e9)"));
        // Hex alpha
        assert_eq!(Some(c(255, 0, 17, 0.0)), parse("#ff001100"));
        assert_eq!(Some(c(255, 255, 255, 1.0)), parse("#ffff"));
    }

    #[test]
    fn getline_split() {
        assert_eq!(split(b"", b','), Vec::<&[u8]>::new());
        assert_eq!(split(b",", b','), vec![b"" as &[u8]]);
        assert_eq!(split(b"1,,2,", b','), vec![b"1" as &[u8], b"", b"2"]);
        // "rgb(,,,)" has three empty fields, each parsing as 0.
        assert_eq!(Some(c(0, 0, 0, 1.0)), parse("rgb(,,,)"));
        assert_eq!(None, parse("rgb(,,)"));
    }
}
