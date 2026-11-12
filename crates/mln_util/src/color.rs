// Ports include/mln/util/color.hpp + src/mln/util/color.cpp.
//
// Note the central quirk, kept on purpose: Color stores a *premultiplied*
// color, all four channels 0..1. `parse` premultiplies; `to_array` /
// `stringify` un-premultiply back to 0..255 channels (with alpha rounded to
// two decimals).

use feature::{Value, ValueObject};
use serde::{Deserialize, Serialize};

use crate::interpolate::interpolate as util_interpolate;
use crate::string::to_string_double;

/// The space a color interpolation runs in: RGB blends the premultiplied
/// channels, LAB and HCL the straight color, OKLCH the premultiplied lightness
/// and chroma.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    Rgb,
    Lab,
    Hcl,
    Oklch,
}

/// Stores a premultiplied color, with all four channels ranging from 0..1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

// --- The D50 Lab conversions GL JS uses (color_spaces.ts, after
// https://observablehq.com/@mbostock/lab-and-rgb). Anonymous namespace in the
// C++ file. ---

const XN: f64 = 0.96422;
const YN: f64 = 1.0;
const ZN: f64 = 0.82521;
const T0: f64 = 4.0 / 29.0;
const T1: f64 = 6.0 / 29.0;
const T2: f64 = 3.0 * T1 * T1;
const T3: f64 = T1 * T1 * T1;

#[derive(Clone, Copy)]
struct Rgb {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

#[derive(Clone, Copy)]
struct Lab {
    l: f64,
    a: f64,
    b: f64,
    alpha: f64,
}

#[derive(Clone, Copy)]
struct Hcl {
    h: f64,
    c: f64,
    l: f64,
    alpha: f64,
}

fn rgb2xyz(x: f64) -> f64 {
    if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
}

fn xyz2lab(t: f64) -> f64 {
    if t > T3 { t.powf(1.0 / 3.0) } else { t / T2 + T0 }
}

fn lab2xyz(t: f64) -> f64 {
    if t > T1 { t * t * t } else { T2 * (t - T0) }
}

fn xyz2rgb(x: f64) -> f64 {
    let x = if x <= 0.00304 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 };
    // std::clamp(x, 0.0, 1.0); both pass NaN and -0.0 through.
    x.clamp(0.0, 1.0)
}

fn constrain_angle(angle: f64) -> f64 {
    let angle = angle % 360.0;
    if angle < 0.0 { angle + 360.0 } else { angle }
}

fn rgb_to_lab(c: Rgb) -> Lab {
    let r = rgb2xyz(c.r);
    let g = rgb2xyz(c.g);
    let b = rgb2xyz(c.b);
    let y = xyz2lab((0.222_504_5 * r + 0.716_878_6 * g + 0.060_616_9 * b) / YN);
    let mut x = y;
    let mut z = y;
    if r != g || g != b {
        x = xyz2lab((0.436_074_7 * r + 0.385_064_9 * g + 0.143_080_4 * b) / XN);
        z = xyz2lab((0.013_932_2 * r + 0.097_104_5 * g + 0.714_173_3 * b) / ZN);
    }
    let l = 116.0 * y - 16.0;
    Lab { l: if l < 0.0 { 0.0 } else { l }, a: 500.0 * (x - y), b: 200.0 * (y - z), alpha: c.a }
}

fn lab_to_rgb(c: Lab) -> Rgb {
    let mut y = (c.l + 16.0) / 116.0;
    let mut x = if c.a.is_nan() { y } else { y + c.a / 500.0 };
    let mut z = if c.b.is_nan() { y } else { y - c.b / 200.0 };
    y = YN * lab2xyz(y);
    x = XN * lab2xyz(x);
    z = ZN * lab2xyz(z);
    Rgb {
        r: xyz2rgb(3.133_856_1 * x - 1.616_866_7 * y - 0.490_614_6 * z),
        g: xyz2rgb(-0.978_768_4 * x + 1.916_141_5 * y + 0.033_454 * z),
        b: xyz2rgb(0.071_945_3 * x - 0.228_991_4 * y + 1.405_242_7 * z),
        a: c.alpha,
    }
}

fn rgb_to_hcl(rgb: Rgb) -> Hcl {
    let lab = rgb_to_lab(rgb);
    let c = (lab.a * lab.a + lab.b * lab.b).sqrt();
    let h = if (c * 10000.0).round() == 0.0 {
        f64::NAN
    } else {
        constrain_angle(lab.b.atan2(lab.a) * 180.0 / std::f64::consts::PI)
    };
    Hcl { h, c, l: lab.l, alpha: lab.alpha }
}

fn hcl_to_rgb(c: Hcl) -> Rgb {
    let h = if c.h.is_nan() { 0.0 } else { c.h * std::f64::consts::PI / 180.0 };
    lab_to_rgb(Lab { l: c.l, a: h.cos() * c.c, b: h.sin() * c.c, alpha: c.alpha })
}

fn straight(color: &Color) -> Rgb {
    if color.a == 0.0 {
        return Rgb { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
    }
    Rgb {
        r: f64::from(color.r / color.a),
        g: f64::from(color.g / color.a),
        b: f64::from(color.b / color.a),
        a: f64::from(color.a),
    }
}

fn premultiplied(c: Rgb) -> Color {
    Color::new((c.r * c.a) as f32, (c.g * c.a) as f32, (c.b * c.a) as f32, c.a as f32)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a * (1.0 - t) + b * t
}

type Vec3 = [f64; 3];
type Mat3 = [Vec3; 3];

// The matrices of https://drafts.csswg.org/css-color-4/#color-conversion-code
const LINEAR_SRGB_TO_XYZ: Mat3 = [
    [506_752.0 / 1_228_815.0, 87881.0 / 245_763.0, 12673.0 / 70218.0],
    [87098.0 / 409_605.0, 175_762.0 / 245_763.0, 12673.0 / 175_545.0],
    [7918.0 / 409_605.0, 87881.0 / 737_289.0, 1_001_167.0 / 1_053_270.0],
];
const XYZ_TO_LINEAR_SRGB: Mat3 = [
    [12831.0 / 3959.0, -329.0 / 214.0, -1974.0 / 3959.0],
    [-851_781.0 / 878_810.0, 1_648_619.0 / 878_810.0, 36519.0 / 878_810.0],
    [705.0 / 12673.0, -2585.0 / 12673.0, 705.0 / 667.0],
];
const XYZ_TO_LMS: Mat3 = [
    [0.819_022_437_996_703, 0.361_906_260_052_890_4, -0.128_873_781_520_987_9],
    [0.032_983_653_932_388_5, 0.929_286_861_586_343_4, 0.036_144_666_350_642_4],
    [0.048_177_189_359_624_2, 0.264_239_531_752_730_8, 0.633_547_828_469_430_9],
];
const LMS_TO_OKLAB: Mat3 = [
    [0.210_454_268_309_314, 0.793_617_774_702_305_4, -0.004_072_043_011_619_3],
    [1.977_998_532_431_168_4, -2.428_592_242_048_58, 0.450_593_709_617_411],
    [0.025_904_042_465_547_8, 0.782_771_712_457_529_6, -0.808_675_754_923_077_4],
];
const OKLAB_TO_LMS: Mat3 = [
    [1.0, 0.396_337_777_376_174_9, 0.215_803_757_309_913_6],
    [1.0, -0.105_561_345_815_658_6, -0.063_854_172_825_813_3],
    [1.0, -0.089_484_177_529_811_9, -1.291_485_548_019_409_2],
];
const LMS_TO_XYZ: Mat3 = [
    [1.226_879_875_845_924_3, -0.557_814_994_460_217_1, 0.281_391_045_665_964_7],
    [-0.040_575_745_214_800_8, 1.112_286_803_280_317, -0.071_711_058_065_516_4],
    [-0.076_372_936_674_660_1, -0.421_493_332_402_243_2, 1.586_924_019_836_781_6],
];

const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;
const RAD_TO_DEG: f64 = 180.0 / std::f64::consts::PI;

// https://drafts.csswg.org/css-color-4/#powerless
const POWERLESS_HUE_EPSILON: f64 = 0.000_004;
// The chroma of 100% in oklch(): https://drafts.csswg.org/css-color-4/#specifying-oklab-oklch
const CHROMA_PERCENT_REFERENCE: f64 = 0.4;
// Keeps the conversion finite: past this chroma, the clipped color stops changing.
const MAX_CHROMA: f64 = 1e6;

#[derive(Clone, Copy)]
struct Oklch {
    l: f64,
    c: f64,
    h: f64,
    alpha: f64,
}

fn multiply(m: &Mat3, v: Vec3) -> Vec3 {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn linear_to_srgb(x: f64) -> f64 {
    let magnitude = x.abs();
    if magnitude > 0.003_130_8 { (1.055 * magnitude.powf(1.0 / 2.4) - 0.055).copysign(x) } else { 12.92 * x }
}

fn srgb_to_oklab(rgb: Vec3) -> Vec3 {
    let lms = multiply(&XYZ_TO_LMS, multiply(&LINEAR_SRGB_TO_XYZ, [rgb2xyz(rgb[0]), rgb2xyz(rgb[1]), rgb2xyz(rgb[2])]));
    multiply(&LMS_TO_OKLAB, [lms[0].cbrt(), lms[1].cbrt(), lms[2].cbrt()])
}

fn oklab_to_srgb(lab: Vec3) -> Vec3 {
    let lms = multiply(&OKLAB_TO_LMS, lab);
    let rgb = multiply(
        &XYZ_TO_LINEAR_SRGB,
        multiply(&LMS_TO_XYZ, [lms[0] * lms[0] * lms[0], lms[1] * lms[1] * lms[1], lms[2] * lms[2] * lms[2]]),
    );
    [linear_to_srgb(rgb[0]), linear_to_srgb(rgb[1]), linear_to_srgb(rgb[2])]
}

fn rgb_to_oklch(rgb: Rgb) -> Oklch {
    let lab = srgb_to_oklab([rgb.r, rgb.g, rgb.b]);
    let c = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
    if c <= POWERLESS_HUE_EPSILON {
        return Oklch { l: lab[0], c: 0.0, h: f64::NAN, alpha: rgb.a };
    }
    Oklch { l: lab[0], c, h: constrain_angle(lab[2].atan2(lab[1]) * RAD_TO_DEG), alpha: rgb.a }
}

// Clips a color outside sRGB channel by channel, as browsers draw one.
fn oklch_to_rgb(color: Oklch) -> Rgb {
    let hue = if color.h.is_nan() { 0.0 } else { color.h * DEG_TO_RAD };
    let chroma = if MAX_CHROMA < color.c { MAX_CHROMA } else { color.c };
    let rgb = oklab_to_srgb([color.l, chroma * hue.cos(), chroma * hue.sin()]);
    Rgb { r: rgb[0].clamp(0.0, 1.0), g: rgb[1].clamp(0.0, 1.0), b: rgb[2].clamp(0.0, 1.0), a: color.alpha }
}

fn is_css_whitespace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

// Reads `none` as 0, or a CSS <number> followed by a `%` that makes 100%
// `percent_reference` or, for a hue, by an angle unit, in degrees within one
// turn.
fn read_oklch_argument(input: &[u8], pos: &mut usize, percent_reference: f64, hue: bool) -> Option<f64> {
    if input[*pos..].starts_with(b"none") {
        *pos += 4;
        return Some(0.0);
    }
    let start = *pos;
    let digits = |pos: &mut usize| {
        let first = *pos;
        while *pos < input.len() && input[*pos].is_ascii_digit() {
            *pos += 1;
        }
        *pos > first
    };
    if *pos < input.len() && (input[*pos] == b'+' || input[*pos] == b'-') {
        *pos += 1;
    }
    let integer = digits(pos);
    if *pos + 1 < input.len() && input[*pos] == b'.' && input[*pos + 1].is_ascii_digit() {
        *pos += 1;
        digits(pos);
    } else if !integer {
        return None;
    }
    if *pos < input.len() && input[*pos] == b'e' {
        let mut exponent = *pos + 1;
        if exponent < input.len() && (input[exponent] == b'+' || input[exponent] == b'-') {
            exponent += 1;
        }
        if exponent < input.len() && input[exponent].is_ascii_digit() {
            *pos = exponent;
            digits(pos);
        }
    }
    let mut value: f64 = std::str::from_utf8(&input[start..*pos]).ok()?.parse().ok()?;
    if hue {
        const UNITS: [(&[u8], f64); 4] = [(b"deg", 1.0), (b"grad", 0.9), (b"rad", RAD_TO_DEG), (b"turn", 360.0)];
        if let Some((unit, degrees)) = UNITS.iter().find(|(unit, _)| input[*pos..].starts_with(unit)) {
            *pos += unit.len();
            // Within one turn in its own unit first: 1e306turn is finite, but
            // not in degrees.
            value = (value % (360.0 / degrees)) * degrees;
        }
    } else if *pos < input.len() && input[*pos] == b'%' {
        *pos += 1;
        value = value / 100.0 * percent_reference;
    }
    value.is_finite().then_some(value)
}

// https://drafts.csswg.org/css-color-4/#specifying-oklab-oklch
fn parse_oklch(text: &str) -> Option<Oklch> {
    const PREFIX: &str = "oklch(";
    let text = text.trim_matches(|c: char| c.is_ascii() && is_css_whitespace(c as u8));
    if text.len() <= PREFIX.len() || !text.get(..PREFIX.len())?.eq_ignore_ascii_case(PREFIX) {
        return None;
    }
    let input = text.to_ascii_lowercase();
    let inside = input.strip_suffix(')')?.as_bytes().get(PREFIX.len()..)?;
    let mut pos = 0;
    let skip_whitespace = |pos: &mut usize| {
        let from = *pos;
        while *pos < inside.len() && is_css_whitespace(inside[*pos]) {
            *pos += 1;
        }
        *pos > from
    };

    skip_whitespace(&mut pos);
    let l = read_oklch_argument(inside, &mut pos, 1.0, false)?;
    if !skip_whitespace(&mut pos) {
        return None;
    }
    let c = read_oklch_argument(inside, &mut pos, CHROMA_PERCENT_REFERENCE, false)?;
    if !skip_whitespace(&mut pos) {
        return None;
    }
    let h = read_oklch_argument(inside, &mut pos, 1.0, true)?;
    skip_whitespace(&mut pos);
    let mut alpha = 1.0;
    if pos < inside.len() && inside[pos] == b'/' {
        pos += 1;
        skip_whitespace(&mut pos);
        alpha = read_oklch_argument(inside, &mut pos, 1.0, false)?;
        skip_whitespace(&mut pos);
    }
    if pos != inside.len() {
        return None;
    }
    Some(Oklch { l: l.clamp(0.0, 1.0), c: if c < 0.0 { 0.0 } else { c }, h: h % 360.0, alpha: alpha.clamp(0.0, 1.0) })
}

impl Color {
    #[must_use]
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        debug_assert!((0.0..=1.0).contains(&r));
        debug_assert!((0.0..=1.0).contains(&g));
        debug_assert!((0.0..=1.0).contains(&b));
        debug_assert!((0.0..=1.0).contains(&a));
        Color { r, g, b, a }
    }

    /// Internal constructor without range asserts (interpolation can
    /// transiently leave 0..1 in the same way the C++ aggregate init does).
    #[must_use]
    pub const fn new_unchecked(r: f32, g: f32, b: f32, a: f32) -> Self {
        Color { r, g, b, a }
    }

    #[must_use]
    pub const fn black() -> Self {
        Color::new_unchecked(0.0, 0.0, 0.0, 1.0)
    }
    #[must_use]
    pub const fn white() -> Self {
        Color::new_unchecked(1.0, 1.0, 1.0, 1.0)
    }
    #[must_use]
    pub const fn red() -> Self {
        Color::new_unchecked(1.0, 0.0, 0.0, 1.0)
    }
    #[must_use]
    pub const fn green() -> Self {
        Color::new_unchecked(0.0, 1.0, 0.0, 1.0)
    }
    #[must_use]
    pub const fn blue() -> Self {
        Color::new_unchecked(0.0, 0.0, 1.0, 1.0)
    }

    /// Parse a CSS color string, returning a premultiplied Color: `oklch()`
    /// here, every other form with the csscolorparser crate.
    #[must_use]
    pub fn parse(s: &str) -> Option<Color> {
        // oklch() is read here, in f64: the crate reads it in f32, and takes
        // some that CSS doesn't, like "oklch(0.6, 0.1, 90)".
        let text = s.trim();
        if text.get(..5).is_some_and(|name| name.eq_ignore_ascii_case("oklch")) {
            return parse_oklch(s).map(|oklch| premultiplied(oklch_to_rgb(oklch)));
        }

        // Deviation: C++ parses with its vendored csscolorparser. The crate
        // gives the same bytes on every color string of the expression and
        // render tests, and also reads CSS Color 4's rgb() and hsl() forms, as
        // GL JS does. It reads more than the style spec's colors, though: hex
        // without its '#', and functions like hwb() and lab(), so those are
        // turned away here.
        if let Some(open) = text.find('(') {
            let name = text[..open].trim_end();
            if !["rgb", "rgba", "hsl", "hsla"].iter().any(|function| name.eq_ignore_ascii_case(function)) {
                return None;
            }
        } else if text.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let css_color = csscolorparser::parse(text).ok()?;
        let [r, g, b, _] = css_color.to_rgba8();

        // Premultiply the color.
        // `css_color->a / 255` is float / int; `r * factor` is int * float.
        let factor: f32 = css_color.a / 255.0f32;
        Some(Color::new(f32::from(r) * factor, f32::from(g) * factor, f32::from(b) * factor, css_color.a))
    }

    #[must_use]
    pub fn interpolate(from: &Color, to: &Color, t: f64, space: ColorSpace) -> Color {
        match space {
            ColorSpace::Rgb => util_interpolate(from, to, t),
            ColorSpace::Lab => {
                let a = rgb_to_lab(straight(from));
                let b = rgb_to_lab(straight(to));
                premultiplied(lab_to_rgb(Lab {
                    l: lerp(a.l, b.l, t),
                    a: lerp(a.a, b.a, t),
                    b: lerp(a.b, b.b, t),
                    alpha: lerp(a.alpha, b.alpha, t),
                }))
            }
            ColorSpace::Hcl => {
                let a = rgb_to_hcl(straight(from));
                let b = rgb_to_hcl(straight(to));
                let mut hue = f64::NAN;
                let mut chroma = f64::NAN;
                if !a.h.is_nan() && !b.h.is_nan() {
                    let mut dh = b.h - a.h;
                    if b.h > a.h && dh > 180.0 {
                        dh -= 360.0;
                    } else if b.h < a.h && a.h - b.h > 180.0 {
                        dh += 360.0;
                    }
                    hue = a.h + t * dh;
                } else if !a.h.is_nan() {
                    hue = a.h;
                    if b.l == 1.0 || b.l == 0.0 {
                        chroma = a.c;
                    }
                } else if !b.h.is_nan() {
                    hue = b.h;
                    if a.l == 1.0 || a.l == 0.0 {
                        chroma = b.c;
                    }
                }
                premultiplied(hcl_to_rgb(Hcl {
                    h: hue,
                    c: if chroma.is_nan() { lerp(a.c, b.c, t) } else { chroma },
                    l: lerp(a.l, b.l, t),
                    alpha: lerp(a.alpha, b.alpha, t),
                }))
            }
            ColorSpace::Oklch => {
                // https://drafts.csswg.org/css-color-4/#interpolation
                let a = rgb_to_oklch(straight(from));
                let b = rgb_to_oklch(straight(to));
                let mut hue_a = if a.h.is_nan() { b.h } else { a.h };
                let mut hue_b = if b.h.is_nan() { a.h } else { b.h };
                if hue_b - hue_a > 180.0 {
                    hue_a += 360.0;
                } else if hue_b - hue_a < -180.0 {
                    hue_b += 360.0;
                }
                let alpha = lerp(a.alpha, b.alpha, t);
                let divisor = if alpha == 0.0 { 1.0 } else { alpha };
                premultiplied(oklch_to_rgb(Oklch {
                    l: lerp(a.l * a.alpha, b.l * b.alpha, t) / divisor,
                    c: lerp(a.c * a.alpha, b.c * b.alpha, t) / divisor,
                    h: lerp(hue_a, hue_b, t),
                    alpha,
                }))
            }
        }
    }

    /// Un-premultiplied `[r*255, g*255, b*255, a]`.
    #[must_use]
    pub fn to_array(&self) -> [f64; 4] {
        if self.a == 0.0 {
            [0.0, 0.0, 0.0, 0.0]
        } else {
            [
                // r * 255 / a: float arithmetic, promoted afterwards.
                f64::from(self.r * 255.0f32 / self.a),
                f64::from(self.g * 255.0f32 / self.a),
                f64::from(self.b * 255.0f32 / self.a),
                f64::from(self.a),
            ]
        }
    }

    /// Port-only: `to_array` in double precision and without rounding alpha.
    fn exact_array(&self) -> [f64; 4] {
        let a = f64::from(self.a);
        let channel = |c: f32| (f64::from(c) / a * 255.0).clamp(0.0, 255.0);
        [channel(self.r), channel(self.g), channel(self.b), a]
    }

    /// Whether the "rgba" expression of these values makes this color: the
    /// premultiplication of `mln_expression`'s `rgba`.
    fn is_made_by(&self, [r, g, b, a]: [f64; 4]) -> bool {
        let premultiplied = |c: f64| (c / 255.0 * a) as f32;
        premultiplied(r) == self.r && premultiplied(g) == self.g && premultiplied(b) == self.b && a as f32 == self.a
    }

    /// `operator std::array<float, 4>`.
    #[must_use]
    pub fn to_float_array(&self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    #[must_use]
    pub fn stringify(&self) -> String {
        // Match GL JS's Color.toString: whole channels, alpha as is.
        let array = self.to_array();
        let channel = |value: f64| to_string_double((value + 0.5).floor());
        format!(
            "rgba({},{},{},{})",
            channel(array[0]),
            channel(array[1]),
            channel(array[2]),
            to_string_double(array[3])
        )
    }

    /// Emit as an rgba expression array for expression serialization (avoids
    /// "Bare objects invalid" parse errors in expression round trips).
    ///
    /// Port-only: when the C++ values (`to_array`) don't make this color
    /// again (the "rgba" expression premultiplies them back), it writes
    /// values that do. `to_array` divides in float, so a serialized layer
    /// could parse back to slightly other colors, and a tile worker in
    /// another wasm instance, which gets its layers that way, would write
    /// other colors into its buckets.
    #[must_use]
    pub fn serialize(&self) -> Value {
        let mut array = self.to_array();
        if !self.is_made_by(array) {
            let exact = self.exact_array();
            if self.is_made_by(exact) {
                array = exact;
            }
        }
        Value::Array(vec![
            Value::String("rgba".to_owned()),
            Value::Double(array[0]),
            Value::Double(array[1]),
            Value::Double(array[2]),
            Value::Double(array[3]),
        ])
    }

    /// Object format for evaluation output: the raw premultiplied channels.
    #[must_use]
    pub fn to_object(&self) -> Value {
        let mut object = ValueObject::new();
        object.insert("r".to_owned(), Value::Double(f64::from(self.r)));
        object.insert("g".to_owned(), Value::Double(f64::from(self.g)));
        object.insert("b".to_owned(), Value::Double(f64::from(self.b)));
        object.insert("a".to_owned(), Value::Double(f64::from(self.a)));
        Value::Object(object)
    }
}

impl std::ops::Mul<f32> for Color {
    type Output = Color;
    fn mul(self, alpha: f32) -> Color {
        debug_assert!((0.0..=1.0).contains(&alpha));
        Color { r: self.r * alpha, g: self.g * alpha, b: self.b * alpha, a: self.a * alpha }
    }
}

#[cfg(test)]
mod tests {
    // Ports (a subset of) test/util/color.test.cpp — extend when porting the
    // full file.
    use super::*;

    #[test]
    fn parse_named() {
        assert_eq!(Color::parse("red"), Some(Color::red()));
        assert_eq!(Color::parse("white"), Some(Color::white()));
        assert_eq!(Color::parse("not-a-color"), None);
    }

    #[test]
    fn parse_premultiplies() {
        let c = Color::parse("rgba(255, 0, 0, 0.5)").unwrap();
        assert!((c.r - 0.5).abs() < 1e-6);
        assert_eq!(c.g, 0.0);
        assert_eq!(c.b, 0.0);
        assert!((c.a - 0.5).abs() < 1e-6);
    }

    #[test]
    fn roundtrip_array() {
        let c = Color::parse("rgba(255, 51, 0, 1)").unwrap();
        let a = c.to_array();
        assert!((a[0] - 255.0).abs() < 1e-3);
        assert!((a[1] - 51.0).abs() < 1e-3);
        assert!((a[2] - 0.0).abs() < 1e-3);
        assert_eq!(a[3], 1.0);
    }

    #[test]
    fn interpolate_rgb_midpoint() {
        let c = Color::interpolate(&Color::black(), &Color::white(), 0.5, ColorSpace::Rgb);
        assert!((c.r - 0.5).abs() < 1e-6);
        assert!((c.a - 1.0).abs() < 1e-6);
    }

    fn serialized(c: &Color) -> [f64; 4] {
        let Value::Array(a) = c.serialize() else { panic!("not an array") };
        let n = |v: &Value| match v {
            Value::Double(d) => *d,
            _ => panic!("not a number"),
        };
        [n(&a[1]), n(&a[2]), n(&a[3]), n(&a[4])]
    }

    /// Port-only: what `serialize` writes makes the same color again,
    /// premultiplied as the "rgba" expression does; and it is the C++
    /// `to_array` when that does.
    #[test]
    fn serialize_makes_the_same_color() {
        for css in ["red", "rgba(255, 0, 0, 0.5)", "rgba(170, 255, 0, 0.6)", "rgba(1, 2, 3, 0.678)", "#123456"] {
            let c = Color::parse(css).unwrap();
            let [r, g, b, a] = serialized(&c);
            let premultiplied = |v: f64| (v / 255.0 * a) as f32;
            assert_eq!((premultiplied(r), premultiplied(g), premultiplied(b), a as f32), (c.r, c.g, c.b, c.a), "{css}");
        }
        assert_eq!(serialized(&Color::parse("rgba(255, 0, 0, 0.5)").unwrap()), [255.0, 0.0, 0.0, 0.5]);
        assert_eq!(serialized(&Color::parse("rgba(1, 2, 3, 0.678)").unwrap())[3], f64::from(0.678f32));
    }
}
