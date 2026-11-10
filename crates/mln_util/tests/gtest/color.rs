// Ports test/util/color.test.cpp.

use mln_util::color::{Color, ColorSpace};

use crate::{assert_double_eq, assert_float_eq, assert_near};

fn test_cases() -> Vec<(&'static str, Option<Color>)> {
    vec![
        // Valid inputs
        ("#000000", Some(Color::new(0.0, 0.0, 0.0, 1.0))),
        ("#FFFFFF", Some(Color::new(1.0, 1.0, 1.0, 1.0))),
        ("#FF0000", Some(Color::new(1.0, 0.0, 0.0, 1.0))),
        ("rgba(255, 0, 0, 1.0)", Some(Color::new(1.0, 0.0, 0.0, 1.0))),
        ("rgb(0, 255, 0)", Some(Color::new(0.0, 1.0, 0.0, 1.0))),
        ("blue", Some(Color::blue())),
        ("red", Some(Color::red())),
        ("rgba(0, 0, 0, 0)", Some(Color::new(0.0, 0.0, 0.0, 0.0))),
        ("#123", Some(Color::new(0.067, 0.133, 0.2, 1.0))), // Short hex format
        ("rgb(-10, 0, 0)", Some(Color::new(0.0, 0.0, 0.0, 1.0))), // Clamped to 0
        ("rgba(300, 0, 0, 1.0)", Some(Color::new(1.0, 0.0, 0.0, 1.0))), // Clamped to 1
        ("rgba(100,100,100,0.2)", Some(Color::new(20.0 / 255.0, 20.0 / 255.0, 20.0 / 255.0, 0.2))),
        ("hsl(120,100%,25%)", Some(Color::new(0.0, 0.5, 0.0, 1.0))),
        ("hsl(240,0%,55%,0.2)", Some(Color::new(0.11, 0.11, 0.11, 0.2))),
        // 4-digit hex with alpha (#RGBA)
        ("#0F0F", Some(Color::new(0.0, 1.0, 0.0, 1.0))),
        ("#F00C", Some(Color::new(0.8, 0.0, 0.0, 0.8))), // Red with ~80% alpha
        ("#123F", Some(Color::new(0.067, 0.133, 0.2, 1.0))), // Same as #123 but with full alpha
        // 8-digit hex with alpha (#RRGGBBAA)
        ("#FF0000FF", Some(Color::new(1.0, 0.0, 0.0, 1.0))), // Red with full alpha
        ("#00FF0080", Some(Color::new(0.0, 0.5, 0.0, 0.5))), // Green with 50% alpha
        ("#0000FF00", Some(Color::new(0.0, 0.0, 0.0, 0.0))), // Blue with 0% alpha (transparent)
        // Invalid inputs
        ("not-a-color", None),
        ("", None),
        ("hsl(120,100%)", None),
        ("hsl(120,100%,50%,1,0)", None),
        ("hsla(120,100%,50%)", None),
    ]
}

// gtest: ColorParse.AllCases
#[test]
fn all_cases() {
    let abs_error = 0.02f32 as f64;
    for (input, expected_result) in test_cases() {
        let result = Color::parse(input);

        match expected_result {
            Some(expected) => {
                // Valid case: Check the values
                let result = result.unwrap_or_else(|| panic!("{input}: no value"));
                assert_near(result.r as f64, expected.r as f64, abs_error);
                assert_near(result.g as f64, expected.g as f64, abs_error);
                assert_near(result.b as f64, expected.b as f64, abs_error);
                assert_near(result.a as f64, expected.a as f64, abs_error);
            }
            None => {
                // Invalid case: Ensure no value is returned
                assert!(result.is_none(), "Unexpected valid result for input: {input}: {result:?}");
            }
        }
    }
}

// gtest: ColorParse.HSLWithAlphaMatchesHSLA
#[test]
fn hsl_with_alpha_matches_hsla() {
    let hsl = Color::parse("hsl(114,55%,73%,0.8)").unwrap();
    let hsla = Color::parse("hsla(114,55%,73%,0.8)").unwrap();

    assert_float_eq(hsl.r, hsla.r);
    assert_float_eq(hsl.g, hsla.g);
    assert_float_eq(hsl.b, hsla.b);
    assert_float_eq(hsl.a, hsla.a);
}

#[track_caller]
fn expect_color(actual: Color, r: f32, g: f32, b: f32, a: f32) {
    let abs_error = 1e-5f32 as f64;
    assert_near(r as f64, actual.r as f64, abs_error);
    assert_near(g as f64, actual.g as f64, abs_error);
    assert_near(b as f64, actual.b as f64, abs_error);
    assert_near(a as f64, actual.a as f64, abs_error);
}

// Expected values come from GL JS's Color.interpolate (maplibre-gl-style-spec), premultiplied.
// gtest: ColorInterpolate.Lab
#[test]
fn lab() {
    let red = Color::parse("rgba(255,0,0,0.5)").unwrap();
    let blue = Color::blue();
    expect_color(Color::interpolate(&red, &blue, 0.0, ColorSpace::Lab), 0.5, 0.0, 0.0, 0.5);
    expect_color(Color::interpolate(&red, &blue, 0.25, ColorSpace::Lab), 0.556105, 0.0, 0.1941047, 0.625);
    expect_color(Color::interpolate(&red, &blue, 0.5, ColorSpace::Lab), 0.5676148, 0.0, 0.4005062, 0.75);
    expect_color(Color::interpolate(&red, &blue, 1.0, ColorSpace::Lab), 0.0, 0.0, 1.0, 1.0);
    expect_color(
        Color::interpolate(&Color::black(), &Color::white(), 0.5, ColorSpace::Lab),
        0.4663266,
        0.4663266,
        0.4663266,
        1.0,
    );
}

// gtest: ColorInterpolate.Hcl
#[test]
fn hcl() {
    let red = Color::parse("rgba(255,0,0,0.5)").unwrap();
    expect_color(Color::interpolate(&red, &Color::blue(), 0.5, ColorSpace::Hcl), 0.7204395, 0.0, 0.3944121, 0.75);
    // A hueless start takes the end's hue.
    expect_color(
        Color::interpolate(&Color::white(), &Color::red(), 0.5, ColorSpace::Hcl),
        1.0,
        0.6241198,
        0.5030824,
        1.0,
    );
    // The hue takes the short way round.
    expect_color(
        Color::interpolate(
            &Color::parse("rgb(255,0,128)").unwrap(),
            &Color::parse("rgb(255,128,0)").unwrap(),
            0.5,
            ColorSpace::Hcl,
        ),
        1.0,
        0.2827256,
        0.298255,
        1.0,
    );
}

// gtest: ColorInterpolate.RgbIsTheExistingInterpolation
#[test]
fn rgb_is_the_existing_interpolation() {
    let red = Color::parse("rgba(255,0,0,0.5)").unwrap();
    let mixed = Color::interpolate(&red, &Color::blue(), 0.5, ColorSpace::Rgb);
    expect_color(mixed, 0.25, 0.0, 0.5, 0.75);
}

// gtest: Color.ToArrayKeepsAlpha
#[test]
fn to_array_keeps_alpha() {
    let color = Color::parse("#ff000080");
    assert!(color.is_some());
    assert_double_eq(f64::from(128.0f32 / 255.0), color.unwrap().to_array()[3]);
}

// Expected strings come from GL JS's Color.toString (maplibre-gl-style-spec).
// gtest: Color.Stringify
#[test]
fn stringify() {
    assert_eq!("rgba(255,0,0,0.125)", Color::parse("rgba(255,0,0,0.125)").unwrap().stringify());
    assert_eq!("rgba(128,0,0,1)", Color::new(0.5, 0.0, 0.0, 1.0).stringify());
    assert_eq!("rgba(0,0,0,0)", Color::new(0.0, 0.0, 0.0, 0.0).stringify());
}
