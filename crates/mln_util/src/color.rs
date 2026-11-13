// Note the central quirk, kept on purpose: Color stores a *premultiplied*
// color, all four channels 0..1. `parse` premultiplies; `to_array` /
// `stringify` un-premultiply back to 0..255 channels (with alpha rounded to
// two decimals).

use std::f32::consts::{PI, TAU};

use csscolorparser::Color as CssColor;
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

// Below these chromas a hue is missing: GL JS's for HCL, which rounds the
// chroma to four decimals, and CSS Color 4's for OKLCH
// (https://drafts.csswg.org/css-color-4/#powerless). The conversions give
// grays a tiny chroma and some hue.
const HCL_POWERLESS_CHROMA: f32 = 0.000_05;
const OKLCH_POWERLESS_CHROMA: f32 = 0.000_004;

fn straight(color: &Color) -> CssColor {
    if color.a == 0.0 {
        return CssColor::new(0.0, 0.0, 0.0, 0.0);
    }
    CssColor::new(color.r / color.a, color.g / color.a, color.b / color.a, color.a)
}

/// Clips each channel to 0..1, as browsers draw a color outside sRGB, and
/// premultiplies.
fn premultiplied(color: &CssColor) -> Color {
    let CssColor { r, g, b, a } = color.clamp();
    Color::new(r * a, g * a, b * a, a)
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

/// A missing hue converts as 0.
fn hue_or_zero(hue: f32) -> f32 {
    if hue.is_nan() { 0.0 } else { hue }
}

/// `[l, c, h, alpha]` in the polar form of CIE Lab, with a NaN hue when it's
/// missing.
fn to_hcl(color: &Color) -> [f32; 4] {
    let [l, c, h, alpha] = straight(color).to_lcha();
    [l.max(0.0), c, if c < HCL_POWERLESS_CHROMA { f32::NAN } else { h }, alpha]
}

/// `[l, c, h, alpha]` in Oklch, with a NaN hue and no chroma when the hue is
/// missing.
fn to_oklch(color: &Color) -> [f32; 4] {
    let [l, c, h, alpha] = straight(color).to_oklcha();
    if c <= OKLCH_POWERLESS_CHROMA { [l, 0.0, f32::NAN, alpha] } else { [l, c, h, alpha] }
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
    premultiplied(&CssColor::from_lcha(lerp(l1, l2, t), chroma, hue_or_zero(hue), lerp(alpha1, alpha2, t)))
}

// https://drafts.csswg.org/css-color-4/#interpolation: a missing hue takes the
// other's, and lightness and chroma are premultiplied.
fn interpolate_oklch(from: &Color, to: &Color, t: f32) -> Color {
    let [l1, c1, h1, alpha1] = to_oklch(from);
    let [l2, c2, h2, alpha2] = to_oklch(to);
    let (h1, h2) = (if h1.is_nan() { h2 } else { h1 }, if h2.is_nan() { h1 } else { h2 });
    let alpha = lerp(alpha1, alpha2, t);
    let divisor = if alpha == 0.0 { 1.0 } else { alpha };
    premultiplied(&CssColor::from_oklcha(
        lerp(l1 * alpha1, l2 * alpha2, t) / divisor,
        lerp(c1 * alpha1, c2 * alpha2, t) / divisor,
        hue_or_zero(h1 + t * hue_difference(h1, h2)),
        alpha,
    ))
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

    /// Parse a CSS color string, returning a premultiplied Color.
    #[must_use]
    pub fn parse(s: &str) -> Option<Color> {
        // Deviation: C++ parses with its vendored csscolorparser. The crate
        // gives the same bytes on every color string of the expression and
        // render tests, and also reads CSS Color 4's rgb() and hsl() forms, as
        // GL JS does. It reads more than the style spec's colors, though: hex
        // without its '#', and functions like hwb() and lab(), so those are
        // turned away here.
        let text = s.trim();
        if let Some(open) = text.find('(') {
            let name = text[..open].trim_end();
            // oklch() keeps its float channels, clipped to sRGB.
            if name.eq_ignore_ascii_case("oklch") {
                return csscolorparser::parse(text).ok().map(|color| premultiplied(&color));
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
            ColorSpace::Rgb => util_interpolate(from, to, t),
            ColorSpace::Lab => premultiplied(&straight(from).interpolate_lab(&straight(to), t as f32)),
            ColorSpace::Hcl => interpolate_hcl(from, to, t as f32),
            ColorSpace::Oklch => interpolate_oklch(from, to, t as f32),
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
