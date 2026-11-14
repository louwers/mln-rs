// Note the central quirk, kept on purpose: Color stores a *premultiplied*
// color, all four channels 0..1. `parse` premultiplies; `to_array` /
// `stringify` un-premultiply back to 0..255 channels (with alpha rounded to
// two decimals). A color outside sRGB is clipped to it channel by channel, and
// keeps its unclipped red, green and blue for interpolation, as the style
// spec's Color does.

use std::f32::consts::{PI, TAU};

use feature::{Value, ValueObject};
use serde::{Deserialize, Serialize};

use crate::interpolate::interpolate as util_interpolate;
use crate::string::to_string_double;

/// The space a color interpolation runs in: RGB blends the premultiplied
/// channels, LAB and HCL the straight color, OKLCH the premultiplied lightness
/// and chroma, and OKLAB the premultiplied lightness, a and b.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    Rgb,
    Lab,
    Hcl,
    Oklch,
    Oklab,
}

/// Stores a premultiplied color, with all four channels ranging from 0..1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
    /// A color outside sRGB: its straight red, green and blue before `r`, `g`
    /// and `b` clipped them to 0..1, which interpolation reads (the style
    /// spec's `unclippedRgb`). `None` inside sRGB.
    #[serde(skip)]
    pub unclipped: Option<[f32; 3]>,
}

// Below these chromas a hue is missing: GL JS's for HCL, which rounds the
// chroma to four decimals, and CSS Color 4's for OKLCH
// (https://drafts.csswg.org/css-color-4/#powerless). The conversions give
// grays a tiny chroma and some hue.
const HCL_POWERLESS_CHROMA: f32 = 0.000_05;
const OKLCH_POWERLESS_CHROMA: f64 = 0.000_004;

// Conversions between sRGB, Oklab and Display P3, as the style spec's
// color_spaces.ts makes them: CSS Color 4's matrices
// (https://drafts.csswg.org/css-color-4/#color-conversion-code) in f64, and
// sRGB's transfer function odd about 0, so a color outside sRGB converts
// with its channels outside 0..1, and back.

type Vector3 = [f64; 3];
type Matrix3 = [Vector3; 3];

const LINEAR_SRGB_TO_XYZ: Matrix3 = [
    [506_752.0 / 1_228_815.0, 87_881.0 / 245_763.0, 12_673.0 / 70_218.0],
    [87_098.0 / 409_605.0, 175_762.0 / 245_763.0, 12_673.0 / 175_545.0],
    [7_918.0 / 409_605.0, 87_881.0 / 737_289.0, 1_001_167.0 / 1_053_270.0],
];
const XYZ_TO_LINEAR_SRGB: Matrix3 = [
    [12_831.0 / 3_959.0, -329.0 / 214.0, -1_974.0 / 3_959.0],
    [-851_781.0 / 878_810.0, 1_648_619.0 / 878_810.0, 36_519.0 / 878_810.0],
    [705.0 / 12_673.0, -2_585.0 / 12_673.0, 705.0 / 667.0],
];
const XYZ_TO_LMS: Matrix3 = [
    [0.819_022_437_996_703, 0.361_906_260_052_890_4, -0.128_873_781_520_987_9],
    [0.032_983_653_932_388_5, 0.929_286_861_586_343_4, 0.036_144_666_350_642_4],
    [0.048_177_189_359_624_2, 0.264_239_531_752_730_8, 0.633_547_828_469_430_9],
];
const LMS_TO_OKLAB: Matrix3 = [
    [0.210_454_268_309_314, 0.793_617_774_702_305_4, -0.004_072_043_011_619_3],
    [1.977_998_532_431_168_4, -2.428_592_242_048_58, 0.450_593_709_617_411],
    [0.025_904_042_465_547_8, 0.782_771_712_457_529_6, -0.808_675_754_923_077_4],
];
const OKLAB_TO_LMS: Matrix3 = [
    [1.0, 0.396_337_777_376_174_9, 0.215_803_757_309_913_6],
    [1.0, -0.105_561_345_815_658_6, -0.063_854_172_825_813_3],
    [1.0, -0.089_484_177_529_811_9, -1.291_485_548_019_409_2],
];
const LMS_TO_XYZ: Matrix3 = [
    [1.226_879_875_845_924_3, -0.557_814_994_460_217_1, 0.281_391_045_665_964_7],
    [-0.040_575_745_214_800_8, 1.112_286_803_280_317, -0.071_711_058_065_516_4],
    [-0.076_372_936_674_660_1, -0.421_493_332_402_243_2, 1.586_924_019_836_781_6],
];
const LINEAR_DISPLAY_P3_TO_XYZ: Matrix3 = [
    [608_311.0 / 1_250_200.0, 189_793.0 / 714_400.0, 198_249.0 / 1_000_160.0],
    [35_783.0 / 156_275.0, 247_089.0 / 357_200.0, 198_249.0 / 2_500_400.0],
    [0.0, 32_229.0 / 714_400.0, 5_220_557.0 / 5_000_800.0],
];

/// Past this chroma an Oklch color clips to the same sRGB color: the style
/// spec's bound, which keeps the conversion finite.
const MAX_CHROMA: f64 = 1e6;
/// The same bound on a Display P3 component.
const MAX_DISPLAY_P3_COMPONENT: f64 = 1e6;

fn multiply(m: &Matrix3, [x, y, z]: Vector3) -> Vector3 {
    [
        m[0][0] * x + m[0][1] * y + m[0][2] * z,
        m[1][0] * x + m[1][1] * y + m[1][2] * z,
        m[2][0] * x + m[2][1] * y + m[2][2] * z,
    ]
}

/// sRGB's (and Display P3's) transfer function, to linear light.
fn srgb_to_linear(x: f64) -> f64 {
    let abs = x.abs();
    if abs <= 0.040_45 { x / 12.92 } else { ((abs + 0.055) / 1.055).powf(2.4).copysign(x) }
}

fn linear_to_srgb(x: f64) -> f64 {
    let abs = x.abs();
    if abs > 0.003_130_8 { (1.055 * abs.powf(1.0 / 2.4) - 0.055).copysign(x) } else { 12.92 * x }
}

fn srgb_to_oklab(rgb: Vector3) -> Vector3 {
    let [l, m, s] = multiply(&XYZ_TO_LMS, multiply(&LINEAR_SRGB_TO_XYZ, rgb.map(srgb_to_linear)));
    multiply(&LMS_TO_OKLAB, [l.cbrt(), m.cbrt(), s.cbrt()])
}

fn oklab_to_srgb(lab: Vector3) -> Vector3 {
    let [l, m, s] = multiply(&OKLAB_TO_LMS, lab);
    multiply(&XYZ_TO_LINEAR_SRGB, multiply(&LMS_TO_XYZ, [l * l * l, m * m * m, s * s * s])).map(linear_to_srgb)
}

/// Oklch with its hue in radians, a missing (NaN) hue converting as 0.
fn oklch_to_srgb(l: f64, c: f64, h: f64) -> Vector3 {
    let hue = if h.is_nan() { 0.0 } else { h };
    let chroma = c.min(MAX_CHROMA);
    oklab_to_srgb([l, chroma * hue.cos(), chroma * hue.sin()])
}

/// Oklab through its polar form, as the style spec converts it, which bounds
/// the chroma.
fn oklab_via_oklch_to_srgb(l: f64, a: f64, b: f64) -> Vector3 {
    oklch_to_srgb(l, a.hypot(b), b.atan2(a))
}

fn display_p3_to_srgb(rgb: Vector3) -> Vector3 {
    let linear = rgb.map(|c| srgb_to_linear(c.clamp(-MAX_DISPLAY_P3_COMPONENT, MAX_DISPLAY_P3_COMPONENT)));
    multiply(&XYZ_TO_LINEAR_SRGB, multiply(&LINEAR_DISPLAY_P3_TO_XYZ, linear)).map(linear_to_srgb)
}

// CIE Lab (D50) and its polar form, HCL, as the style spec converts them
// (https://observablehq.com/@mbostock/lab-and-rgb), with sRGB's transfer
// function odd about 0 both ways.
const LAB_XN: f64 = 0.964_22;
const LAB_YN: f64 = 1.0;
const LAB_ZN: f64 = 0.825_21;
const LAB_T0: f64 = 4.0 / 29.0;
const LAB_T1: f64 = 6.0 / 29.0;
const LAB_T2: f64 = 3.0 * LAB_T1 * LAB_T1;
const LAB_T3: f64 = LAB_T1 * LAB_T1 * LAB_T1;

fn xyz_to_lab(t: f64) -> f64 {
    if t > LAB_T3 { t.powf(1.0 / 3.0) } else { t / LAB_T2 + LAB_T0 }
}

fn lab_to_xyz(t: f64) -> f64 {
    if t > LAB_T1 { t * t * t } else { LAB_T2 * (t - LAB_T0) }
}

/// The style spec's own encoding back to sRGB, whose linear part ends at
/// 0.00304.
fn xyz_to_srgb(x: f64) -> f64 {
    let abs = x.abs();
    if abs <= 0.003_04 { 12.92 * x } else { (1.055 * abs.powf(1.0 / 2.4) - 0.055).copysign(x) }
}

fn srgb_to_lab(rgb: Vector3) -> Vector3 {
    let [r, g, b] = rgb.map(srgb_to_linear);
    let y = xyz_to_lab((0.222_504_5 * r + 0.716_878_6 * g + 0.060_616_9 * b) / LAB_YN);
    let (x, z) = if r == g && g == b {
        (y, y)
    } else {
        (
            xyz_to_lab((0.436_074_7 * r + 0.385_064_9 * g + 0.143_080_4 * b) / LAB_XN),
            xyz_to_lab((0.013_932_2 * r + 0.097_104_5 * g + 0.714_173_3 * b) / LAB_ZN),
        )
    };
    [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
}

fn lab_to_srgb([l, a, b]: Vector3) -> Vector3 {
    let y = (l + 16.0) / 116.0;
    let x = if a.is_nan() { y } else { y + a / 500.0 };
    let z = if b.is_nan() { y } else { y - b / 200.0 };
    let (x, y, z) = (LAB_XN * lab_to_xyz(x), LAB_YN * lab_to_xyz(y), LAB_ZN * lab_to_xyz(z));
    [
        xyz_to_srgb(3.133_856_1 * x - 1.616_866_7 * y - 0.490_614_6 * z),
        xyz_to_srgb(-0.978_768_4 * x + 1.916_141_5 * y + 0.033_454 * z),
        xyz_to_srgb(0.071_945_3 * x - 0.228_991_4 * y + 1.405_242_7 * z),
    ]
}

/// HCL with its hue in radians, a missing (NaN) hue converting as 0.
fn hcl_to_srgb(l: f64, c: f64, h: f64) -> Vector3 {
    let hue = if h.is_nan() { 0.0 } else { h };
    lab_to_srgb([l, hue.cos() * c, hue.sin() * c])
}

// CSS Color 4's oklch(), oklab() and color(display-p3 ...), read as the style
// spec's parse_css_color.ts reads them: space-separated numbers, percentages
// and `none` (0), an optional alpha after a slash, and the hue's angle units.

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

/// `oklch()`, `oklab()` or `color(display-p3 ...)`, as the style spec reads them.
fn parse_css_color_4(text: &str) -> Option<Color> {
    let text = text.to_ascii_lowercase();
    let arguments = |function: &str| text.strip_prefix(function).and_then(|rest| rest.strip_suffix(')'));
    if let Some(arguments) = arguments("oklch(") {
        let [l, c, h, alpha] = css_components(arguments, [1.0, 0.4, 1.0], true)?;
        let rgb = oklch_to_srgb(l.clamp(0.0, 1.0), c.max(0.0), (h % 360.0).to_radians());
        return Some(Color::from_straight(rgb, alpha.clamp(0.0, 1.0)));
    }
    if let Some(arguments) = arguments("oklab(") {
        let [l, a, b, alpha] = css_components(arguments, [1.0, 0.4, 0.4], false)?;
        return Some(Color::from_straight(oklab_via_oklch_to_srgb(l.clamp(0.0, 1.0), a, b), alpha.clamp(0.0, 1.0)));
    }
    // color() reads only Display P3.
    let p3 = arguments("color(")?.trim_start().strip_prefix("display-p3")?;
    if !p3.starts_with(char::is_whitespace) {
        return None;
    }
    let [r, g, b, alpha] = css_components(p3, [1.0; 3], false)?;
    Some(Color::from_straight(display_p3_to_srgb([r, g, b]), alpha.clamp(0.0, 1.0)))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a * (1.0 - t) + b * t
}

/// How far `to` is from `from` the shorter way around, in radians.
fn hue_difference(from: f32, to: f32) -> f32 {
    let difference = to - from;
    if difference > PI {
        difference - TAU
    } else if difference < -PI {
        difference + TAU
    } else {
        difference
    }
}

/// `[l, c, h, alpha]` in the polar form of CIE Lab, the hue in radians, NaN
/// when it's missing. A color darker than black keeps its lightness below 0.
fn to_hcl(color: &Color) -> [f32; 4] {
    let [r, g, b, alpha] = color.straight_rgba();
    let [l, a, b] = srgb_to_lab([r, g, b].map(f64::from));
    let c = (a * a + b * b).sqrt() as f32;
    [l as f32, c, if c < HCL_POWERLESS_CHROMA { f32::NAN } else { b.atan2(a) as f32 }, alpha]
}

/// The style spec's LAB interpolation: lightness, a, b and alpha, straight.
fn interpolate_lab(from: &Color, to: &Color, t: f64) -> Color {
    let [r1, g1, b1, alpha1] = from.straight_rgba().map(f64::from);
    let [r2, g2, b2, alpha2] = to.straight_rgba().map(f64::from);
    let (lab1, lab2) = (srgb_to_lab([r1, g1, b1]), srgb_to_lab([r2, g2, b2]));
    let mix = |a: f64, b: f64| a + t * (b - a);
    let rgb = lab_to_srgb([mix(lab1[0], lab2[0]), mix(lab1[1], lab2[1]), mix(lab1[2], lab2[2])]);
    Color::from_straight(rgb, mix(alpha1, alpha2))
}

/// `[l, a, b, alpha]` in Oklab.
fn to_oklab(color: &Color) -> [f32; 4] {
    let [r, g, b, alpha] = color.straight_rgba();
    let [l, a, b] = srgb_to_oklab([r, g, b].map(f64::from));
    [l as f32, a as f32, b as f32, alpha]
}

/// `[l, c, h, alpha]` in Oklch, the hue in radians: NaN, with no chroma, when
/// it's missing.
fn to_oklch(color: &Color) -> [f32; 4] {
    let [r, g, b, alpha] = color.straight_rgba();
    let [l, a, b] = srgb_to_oklab([r, g, b].map(f64::from));
    let c = (a * a + b * b).sqrt();
    if c <= OKLCH_POWERLESS_CHROMA {
        [l as f32, 0.0, f32::NAN, alpha]
    } else {
        [l as f32, c as f32, b.atan2(a) as f32, alpha]
    }
}

// GL JS's interpolateHcl: a missing hue takes the other's, and so does the
// chroma when the other is black or white.
fn interpolate_hcl(from: &Color, to: &Color, t: f32) -> Color {
    let [l1, c1, h1, alpha1] = to_hcl(from);
    let [l2, c2, h2, alpha2] = to_hcl(to);
    let mut chroma = lerp(c1, c2, t);
    let hue = match (h1.is_nan(), h2.is_nan()) {
        (false, false) => h1 + t * hue_difference(h1, h2),
        (false, true) => {
            if l2 == 1.0 || l2 == 0.0 {
                chroma = c1;
            }
            h1
        }
        (true, false) => {
            if l1 == 1.0 || l1 == 0.0 {
                chroma = c2;
            }
            h2
        }
        (true, true) => f32::NAN,
    };
    let rgb = hcl_to_srgb(f64::from(lerp(l1, l2, t)), f64::from(chroma), f64::from(hue));
    Color::from_straight(rgb, f64::from(lerp(alpha1, alpha2, t)))
}

// https://drafts.csswg.org/css-color-4/#interpolation: a missing hue takes the
// other's, and lightness and chroma are premultiplied.
fn interpolate_oklch(from: &Color, to: &Color, t: f32) -> Color {
    let [l1, c1, h1, alpha1] = to_oklch(from);
    let [l2, c2, h2, alpha2] = to_oklch(to);
    let (h1, h2) = (if h1.is_nan() { h2 } else { h1 }, if h2.is_nan() { h1 } else { h2 });
    let alpha = lerp(alpha1, alpha2, t);
    let divisor = if alpha == 0.0 { 1.0 } else { alpha };
    let rgb = oklch_to_srgb(
        f64::from(lerp(l1 * alpha1, l2 * alpha2, t) / divisor),
        f64::from(lerp(c1 * alpha1, c2 * alpha2, t) / divisor),
        f64::from(h1 + t * hue_difference(h1, h2)),
    );
    Color::from_straight(rgb, f64::from(alpha))
}

// https://drafts.csswg.org/css-color-4/#interpolation: lightness, a and b
// are premultiplied, and there is no hue to fix up.
fn interpolate_oklab(from: &Color, to: &Color, t: f32) -> Color {
    let [l1, a1, b1, alpha1] = to_oklab(from);
    let [l2, a2, b2, alpha2] = to_oklab(to);
    let alpha = lerp(alpha1, alpha2, t);
    let divisor = if alpha == 0.0 { 1.0 } else { alpha };
    let rgb = oklab_via_oklch_to_srgb(
        f64::from(lerp(l1 * alpha1, l2 * alpha2, t) / divisor),
        f64::from(lerp(a1 * alpha1, a2 * alpha2, t) / divisor),
        f64::from(lerp(b1 * alpha1, b2 * alpha2, t) / divisor),
    );
    Color::from_straight(rgb, f64::from(alpha))
}

/// RGB interpolation, which blends the premultiplied channels, of a color
/// outside sRGB: its unclipped channels blend.
fn interpolate_unclipped_rgb(from: &Color, to: &Color, t: f64) -> Color {
    let [r1, g1, b1, alpha1] = from.straight_rgba().map(f64::from);
    let [r2, g2, b2, alpha2] = to.straight_rgba().map(f64::from);
    let alpha = alpha1 * (1.0 - t) + alpha2 * t;
    if alpha == 0.0 {
        return Color::new(0.0, 0.0, 0.0, 0.0);
    }
    let blend = |c1: f64, c2: f64| (c1 * alpha1 * (1.0 - t) + c2 * alpha2 * t) / alpha;
    Color::from_straight([blend(r1, r2), blend(g1, g2), blend(b1, b2)], alpha)
}

impl Color {
    #[must_use]
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        debug_assert!((0.0..=1.0).contains(&r));
        debug_assert!((0.0..=1.0).contains(&g));
        debug_assert!((0.0..=1.0).contains(&b));
        debug_assert!((0.0..=1.0).contains(&a));
        Color { r, g, b, a, unclipped: None }
    }

    /// Internal constructor without range asserts (interpolation can
    /// transiently leave 0..1 in the same way the C++ aggregate init does).
    #[must_use]
    pub const fn new_unchecked(r: f32, g: f32, b: f32, a: f32) -> Self {
        Color { r, g, b, a, unclipped: None }
    }

    /// A straight color, its channels clipped to 0..1 and premultiplied, and
    /// kept unclipped when any is outside: the style spec's `new Color(r, g,
    /// b, alpha, false)`.
    fn from_straight(rgb: [f64; 3], alpha: f64) -> Color {
        let a = alpha as f32;
        let [r, g, b] = rgb.map(|c| c.clamp(0.0, 1.0) as f32);
        let mut color = Color::new(r * a, g * a, b * a, a);
        if rgb.iter().any(|c| !(0.0..=1.0).contains(c)) {
            color.unclipped = Some(rgb.map(|c| c as f32));
        }
        color
    }

    /// The straight color, unclipped (the style spec's `unclippedRgb`).
    fn straight_rgba(&self) -> [f32; 4] {
        match self.unclipped {
            Some([r, g, b]) => [r, g, b, self.a],
            None if self.a == 0.0 => [0.0; 4],
            None => [self.r / self.a, self.g / self.a, self.b / self.a, self.a],
        }
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

    /// Parse a CSS color string, returning a premultiplied Color.
    #[must_use]
    pub fn parse(s: &str) -> Option<Color> {
        // Deviation: C++ parses with its vendored csscolorparser. The crate
        // gives the same bytes on every color string of the expression and
        // render tests, and also reads CSS Color 4's rgb() and hsl() forms, as
        // GL JS does. It reads more than the style spec's colors, though: hex
        // without its '#', and functions like hwb() and lab(), so those are
        // turned away here. oklch(), oklab() and color() are read here as
        // the style spec reads them, keeping their float channels.
        let text = s.trim();
        if let Some(open) = text.find('(') {
            let name = text[..open].trim_end();
            if ["oklch", "oklab", "color"].iter().any(|function| name.eq_ignore_ascii_case(function)) {
                return parse_css_color_4(text);
            }
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
            ColorSpace::Rgb if from.unclipped.is_some() || to.unclipped.is_some() => {
                interpolate_unclipped_rgb(from, to, t)
            }
            ColorSpace::Rgb => util_interpolate(from, to, t),
            ColorSpace::Lab => interpolate_lab(from, to, t),
            ColorSpace::Hcl => interpolate_hcl(from, to, t as f32),
            ColorSpace::Oklch => interpolate_oklch(from, to, t as f32),
            ColorSpace::Oklab => interpolate_oklab(from, to, t as f32),
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
        // The straight color, and so its unclipped channels, stay as they are.
        Color { r: self.r * alpha, g: self.g * alpha, b: self.b * alpha, a: self.a * alpha, ..self }
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
