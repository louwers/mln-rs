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
/// channels, LAB and HCL the straight color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    Rgb,
    Lab,
    Hcl,
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

    /// Parse a CSS color string (vendor/csscolorparser), returning a
    /// premultiplied Color. Like the C++ constructor, this asserts (in debug
    /// builds) that the alpha is in 0..1, which a NaN alpha (`rgba(0,0,0,nan)`)
    /// is not.
    #[must_use]
    pub fn parse(s: &str) -> Option<Color> {
        let css_color = csscolorparser::parse(s)?;

        // Premultiply the color.
        // `css_color->a / 255` is float / int; `r * factor` is int * float.
        let factor: f32 = css_color.a / 255.0f32;
        Some(Color::new(
            f32::from(css_color.r) * factor,
            f32::from(css_color.g) * factor,
            f32::from(css_color.b) * factor,
            css_color.a,
        ))
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
