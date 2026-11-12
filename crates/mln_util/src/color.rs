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
    // std::clamp(x, 0.0, 1.0): `x < lo ? lo : hi < x ? hi : x` (NaN and -0.0
    // pass through).
    if x < 0.0 {
        0.0
    } else if 1.0 < x {
        1.0
    } else {
        x
    }
}

fn constrain_angle(angle: f64) -> f64 {
    let angle = angle % 360.0;
    if angle < 0.0 { angle + 360.0 } else { angle }
}

fn rgb_to_lab(c: Rgb) -> Lab {
    let r = rgb2xyz(c.r);
    let g = rgb2xyz(c.g);
    let b = rgb2xyz(c.b);
    let y = xyz2lab((0.2225045 * r + 0.7168786 * g + 0.0606169 * b) / YN);
    let mut x = y;
    let mut z = y;
    if r != g || g != b {
        x = xyz2lab((0.4360747 * r + 0.3850649 * g + 0.1430804 * b) / XN);
        z = xyz2lab((0.0139322 * r + 0.0971045 * g + 0.7141733 * b) / ZN);
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
        r: xyz2rgb(3.1338561 * x - 1.6168667 * y - 0.4906146 * z),
        g: xyz2rgb(-0.9787684 * x + 1.9161415 * y + 0.033454 * z),
        b: xyz2rgb(0.0719453 * x - 0.2289914 * y + 1.4052427 * z),
        a: c.alpha,
    }
}

fn rgb_to_hcl(rgb: Rgb) -> Hcl {
    let lab = rgb_to_lab(rgb);
    let c = (lab.a * lab.a + lab.b * lab.b).sqrt();
    let h = if (c * 10000.0).round() != 0.0 {
        constrain_angle(lab.b.atan2(lab.a) * 180.0 / std::f64::consts::PI)
    } else {
        f64::NAN
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
        r: (color.r / color.a) as f64,
        g: (color.g / color.a) as f64,
        b: (color.b / color.a) as f64,
        a: color.a as f64,
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
    [506752.0 / 1228815.0, 87881.0 / 245763.0, 12673.0 / 70218.0],
    [87098.0 / 409605.0, 175762.0 / 245763.0, 12673.0 / 175545.0],
    [7918.0 / 409605.0, 87881.0 / 737289.0, 1001167.0 / 1053270.0],
];
const XYZ_TO_LINEAR_SRGB: Mat3 = [
    [12831.0 / 3959.0, -329.0 / 214.0, -1974.0 / 3959.0],
    [-851781.0 / 878810.0, 1648619.0 / 878810.0, 36519.0 / 878810.0],
    [705.0 / 12673.0, -2585.0 / 12673.0, 705.0 / 667.0],
];
const XYZ_TO_LMS: Mat3 = [
    [0.8190224379967030, 0.3619062600528904, -0.1288737815209879],
    [0.0329836539323885, 0.9292868615863434, 0.0361446663506424],
    [0.0481771893596242, 0.2642395317527308, 0.6335478284694309],
];
const LMS_TO_OKLAB: Mat3 = [
    [0.2104542683093140, 0.7936177747023054, -0.0040720430116193],
    [1.9779985324311684, -2.4285922420485799, 0.4505937096174110],
    [0.0259040424655478, 0.7827717124575296, -0.8086757549230774],
];
const OKLAB_TO_LMS: Mat3 = [
    [1.0, 0.3963377773761749, 0.2158037573099136],
    [1.0, -0.1055613458156586, -0.0638541728258133],
    [1.0, -0.0894841775298119, -1.2914855480194092],
];
const LMS_TO_XYZ: Mat3 = [
    [1.2268798758459243, -0.5578149944602171, 0.2813910456659647],
    [-0.0405757452148008, 1.1122868032803170, -0.0717110580655164],
    [-0.0763729366746601, -0.4214933324022432, 1.5869240198367816],
];

const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;
const RAD_TO_DEG: f64 = 180.0 / std::f64::consts::PI;

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
    if magnitude > 0.0031308 { (1.055 * magnitude.powf(1.0 / 2.4) - 0.055).copysign(x) } else { 12.92 * x }
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
    // https://drafts.csswg.org/css-color-4/#powerless
    if c <= 0.000004 {
        return Oklch { l: lab[0], c: 0.0, h: f64::NAN, alpha: rgb.a };
    }
    Oklch { l: lab[0], c, h: constrain_angle(lab[2].atan2(lab[1]) * RAD_TO_DEG), alpha: rgb.a }
}

fn is_in_srgb_gamut(rgb: Vec3) -> bool {
    rgb.iter().all(|&x| x >= 0.0 && x <= 1.0)
}

fn clip_to_srgb_gamut(rgb: Vec3) -> Vec3 {
    rgb.map(|x| x.clamp(0.0, 1.0))
}

// Maps a color outside sRGB to a lower chroma at the same lightness and hue,
// https://drafts.csswg.org/css-color-4/#binary-search-gamut-map-with-local-minde
fn oklch_to_rgb(color: Oklch) -> Rgb {
    if color.l >= 1.0 {
        return Rgb { r: 1.0, g: 1.0, b: 1.0, a: color.alpha };
    }
    if color.l <= 0.0 {
        return Rgb { r: 0.0, g: 0.0, b: 0.0, a: color.alpha };
    }
    let hue = if color.h.is_nan() { 0.0 } else { color.h * DEG_TO_RAD };
    let cos_hue = hue.cos();
    let sin_hue = hue.sin();
    let to_srgb = |chroma: f64| oklab_to_srgb([color.l, chroma * cos_hue, chroma * sin_hue]);
    let delta_eok = |rgb: Vec3, chroma: f64| {
        let lab = srgb_to_oklab(rgb);
        let dl = color.l - lab[0];
        let da = chroma * cos_hue - lab[1];
        let db = chroma * sin_hue - lab[2];
        (dl * dl + da * da + db * db).sqrt()
    };
    const JND: f64 = 0.02;
    const EPSILON: f64 = 0.0001;

    let rgb = to_srgb(color.c);
    if is_in_srgb_gamut(rgb) {
        return Rgb { r: rgb[0], g: rgb[1], b: rgb[2], a: color.alpha };
    }
    let mut clipped = clip_to_srgb_gamut(rgb);
    if delta_eok(clipped, color.c) >= JND {
        let mut min = 0.0;
        let mut max = color.c;
        let mut min_in_gamut = true;
        while max - min > EPSILON {
            let chroma = (min + max) / 2.0;
            let current = to_srgb(chroma);
            if min_in_gamut && is_in_srgb_gamut(current) {
                min = chroma;
                continue;
            }
            clipped = clip_to_srgb_gamut(current);
            let e = delta_eok(clipped, chroma);
            if e < JND {
                if JND - e < EPSILON {
                    break;
                }
                min_in_gamut = false;
                min = chroma;
            } else {
                max = chroma;
            }
        }
    }
    Rgb { r: clipped[0], g: clipped[1], b: clipped[2], a: color.alpha }
}

fn is_css_whitespace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

// Reads `none` as 0, or a CSS <number> followed by a `%` that makes 100%
// `percent_reference` or, for a hue, by an angle unit, in degrees.
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
            value *= degrees;
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
    let c = read_oklch_argument(inside, &mut pos, 0.4, false)?;
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
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        debug_assert!((0.0..=1.0).contains(&r));
        debug_assert!((0.0..=1.0).contains(&g));
        debug_assert!((0.0..=1.0).contains(&b));
        debug_assert!((0.0..=1.0).contains(&a));
        Color { r, g, b, a }
    }

    /// Internal constructor without range asserts (interpolation can
    /// transiently leave 0..1 in the same way the C++ aggregate init does).
    pub const fn new_unchecked(r: f32, g: f32, b: f32, a: f32) -> Self {
        Color { r, g, b, a }
    }

    pub const fn black() -> Self {
        Color::new_unchecked(0.0, 0.0, 0.0, 1.0)
    }
    pub const fn white() -> Self {
        Color::new_unchecked(1.0, 1.0, 1.0, 1.0)
    }
    pub const fn red() -> Self {
        Color::new_unchecked(1.0, 0.0, 0.0, 1.0)
    }
    pub const fn green() -> Self {
        Color::new_unchecked(0.0, 1.0, 0.0, 1.0)
    }
    pub const fn blue() -> Self {
        Color::new_unchecked(0.0, 0.0, 1.0, 1.0)
    }

    /// Parse a CSS color string (vendor/csscolorparser), returning a
    /// premultiplied Color. Like the C++ constructor, this asserts (in debug
    /// builds) that the alpha is in 0..1, which a NaN alpha (`rgba(0,0,0,nan)`)
    /// is not.
    pub fn parse(s: &str) -> Option<Color> {
        if let Some(oklch) = parse_oklch(s) {
            return Some(premultiplied(oklch_to_rgb(oklch)));
        }

        let css_color = csscolorparser::parse(s)?;

        // Premultiply the color.
        // `css_color->a / 255` is float / int; `r * factor` is int * float.
        let factor: f32 = css_color.a / 255.0f32;
        Some(Color::new(
            css_color.r as f32 * factor,
            css_color.g as f32 * factor,
            css_color.b as f32 * factor,
            css_color.a,
        ))
    }

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
    pub fn to_array(&self) -> [f64; 4] {
        if self.a == 0.0 {
            [0.0, 0.0, 0.0, 0.0]
        } else {
            [
                // r * 255 / a: float arithmetic, promoted afterwards.
                (self.r * 255.0f32 / self.a) as f64,
                (self.g * 255.0f32 / self.a) as f64,
                (self.b * 255.0f32 / self.a) as f64,
                self.a as f64,
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
    /// premultiplication of mln_expression's `rgba`.
    fn is_made_by(&self, [r, g, b, a]: [f64; 4]) -> bool {
        let premultiplied = |c: f64| (c / 255.0 * a) as f32;
        premultiplied(r) == self.r && premultiplied(g) == self.g && premultiplied(b) == self.b && a as f32 == self.a
    }

    /// `operator std::array<float, 4>`.
    pub fn to_float_array(&self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// `Color(const float rgba[4])`.
    pub fn from_array(rgba: [f32; 4]) -> Self {
        Color::new(rgba[0], rgba[1], rgba[2], rgba[3])
    }

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
    pub fn to_object(&self) -> Value {
        let mut object = ValueObject::new();
        object.insert("r".to_owned(), Value::Double(self.r as f64));
        object.insert("g".to_owned(), Value::Double(self.g as f64));
        object.insert("b".to_owned(), Value::Double(self.b as f64));
        object.insert("a".to_owned(), Value::Double(self.a as f64));
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
