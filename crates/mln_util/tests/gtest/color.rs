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
        // Deviation: C++ wants hsla()'s alpha. The csscolorparser crate reads
        // hsla() without one as hsl(), as CSS Color 4 and GL JS do.
        ("hsla(120,100%,50%)", Some(Color::new(0.0, 1.0, 0.0, 1.0))),
    ]
}

// gtest: ColorParse.AllCases
#[test]
fn all_cases() {
    let abs_error = f64::from(0.02f32);
    for (input, expected_result) in test_cases() {
        let result = Color::parse(input);

        match expected_result {
            Some(expected) => {
                // Valid case: Check the values
                let result = result.unwrap_or_else(|| panic!("{input}: no value"));
                assert_near(f64::from(result.r), f64::from(expected.r), abs_error);
                assert_near(f64::from(result.g), f64::from(expected.g), abs_error);
                assert_near(f64::from(result.b), f64::from(expected.b), abs_error);
                assert_near(f64::from(result.a), f64::from(expected.a), abs_error);
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
    let abs_error = f64::from(1e-5f32);
    assert_near(f64::from(r), f64::from(actual.r), abs_error);
    assert_near(f64::from(g), f64::from(actual.g), abs_error);
    assert_near(f64::from(b), f64::from(actual.b), abs_error);
    assert_near(f64::from(a), f64::from(actual.a), abs_error);
}

// Expected values come from colorjs.io, by the CSS Color 4 editors, premultiplied.
// gtest: ColorParse.Oklch
#[test]
fn oklch() {
    expect_color(Color::parse("oklch(70% 0.1 30)").unwrap(), 0.836_736_1, 0.526_626_5, 0.475_198_2, 1.0);
    expect_color(Color::parse("oklch(0.8 0.05 160 / 0.75)").unwrap(), 0.478_585_3, 0.588_708, 0.525_818_2, 0.75);
    for input in [
        "oklch(0.6 0.1 90)",
        "oklch(60% 25% 90deg)",
        "oklch(0.6 0.1 100grad)",
        "oklch(0.6 0.1 1.5707963267948966rad)",
        "oklch(0.6 0.1 0.25turn)",
        "oklch(0.6 0.1 450)",
        "oklch(0.6 0.1 -270deg)",
        "OKLCH(60% 0.1 90DEG)",
        "oklch(6e-1 1E-1 +9e1)",
        "oklch(+.6 .1 90 / 1)",
        " oklch(  0.6\t0.1\n90  /  100% ) ",
    ] {
        let color = Color::parse(input).unwrap_or_else(|| panic!("{input}: no value"));
        expect_color(color, 0.593_718_5, 0.490_827_5, 0.190_116_1, 1.0);
    }
    expect_color(Color::parse("oklch(0.6 0.1 90 / 50%)").unwrap(), 0.296_859_3, 0.245_413_8, 0.095_058_07, 0.5);
}

// gtest: ColorParse.OklchClampsAndReadsNoneAsZero
#[test]
fn oklch_clamps_and_reads_none_as_zero() {
    let darkest = Color::parse("oklch(0 0.1 30)").unwrap();
    // Deviation: C++ clamps the lightness to 1, as CSS Color 4 does. The
    // csscolorparser crate clamps it only to 0, so this clips to white.
    expect_color(Color::parse("oklch(1.2 0.1 30)").unwrap(), 1.0, 1.0, 1.0, 1.0);
    expect_color(Color::parse("oklch(-0.1 0.1 30)").unwrap(), darkest.r, darkest.g, darkest.b, darkest.a);
    expect_color(Color::parse("oklch(0.6 -0.1 30)").unwrap(), 0.502_110_7, 0.502_110_7, 0.502_110_7, 1.0);
    expect_color(Color::parse("oklch(0.6 0.1 90 / 1.5)").unwrap(), 0.593_718_5, 0.490_827_5, 0.190_116_1, 1.0);
    expect_color(Color::parse("oklch(0.6 0.1 90 / -1)").unwrap(), 0.0, 0.0, 0.0, 0.0);
    let zero_lightness = Color::parse("oklch(0 0.1 90)").unwrap();
    expect_color(
        Color::parse("oklch(none 0.1 90)").unwrap(),
        zero_lightness.r,
        zero_lightness.g,
        zero_lightness.b,
        zero_lightness.a,
    );
    expect_color(Color::parse("oklch(0.6 none 90)").unwrap(), 0.502_110_7, 0.502_110_7, 0.502_110_7, 1.0);
    let zero_hue = Color::parse("oklch(0.6 0.1 0)").unwrap();
    expect_color(Color::parse("oklch(0.6 0.1 none)").unwrap(), zero_hue.r, zero_hue.g, zero_hue.b, zero_hue.a);
    expect_color(Color::parse("oklch(0.6 0.1 90 / none)").unwrap(), 0.0, 0.0, 0.0, 0.0);
}

// Deviation: C++ reads oklch() in double precision and wraps these within one
// turn. The csscolorparser crate reads it in f32, where they overflow.
// gtest: ColorParse.OklchWrapsAHugeHueAngleBeforeConvertingIt
#[test]
fn oklch_wraps_a_huge_hue_angle_before_converting_it() {
    assert!(Color::parse("oklch(0.6 0.1 1e306turn)").is_none());
    assert!(Color::parse("oklch(0.6 0.1 1e307rad)").is_none());
}

// gtest: ColorParse.OklchOutsideSrgbIsClipped
#[test]
fn oklch_outside_srgb_is_clipped() {
    expect_color(Color::parse("oklch(0.7 0.3 140)").unwrap(), 0.0, 0.771_585_4, 0.0, 1.0);
    expect_color(Color::parse("oklch(0.9 0.3 300)").unwrap(), 1.0, 0.625_509_3, 1.0, 1.0);
    expect_color(Color::parse("oklch(0.3 0.25 30)").unwrap(), 0.509_169_5, 0.0, 0.0, 1.0);
    // Deviation: C++ converts a huge chroma as chroma 1e6, which gives green.
    // The csscolorparser crate reads it in f32, where 1e110 overflows.
    assert!(Color::parse("oklch(0.6 1e110 90)").is_none());
}

// gtest: ColorParse.OklchInvalid
#[test]
fn oklch_invalid() {
    for input in [
        "oklch()",
        "oklch(0.6 0.1)",
        "oklch(0.6 0.1 90 /)",
        "oklch(0.6 0.1 90 / 0.5 / 0.5)",
        "oklch(0.6 0.1 90%)",
        "oklch(0.6 0.1 90 deg)",
        "oklch(0.6 0.1 90grads)",
        "oklch(0.6 0.1 nonedeg)",
        "oklch(0.6deg 0.1 90)",
        "oklch(0.6 0.1deg 90)",
        "oklch(0.6 0.1 --90)",
        "oklch(0.6 0.1 9e)",
        "oklch(0.6 0.1 1e999)",
        "oklch(0.6 0.1 90",
        "oklch(0.6 0.1 90))",
    ] {
        assert!(Color::parse(input).is_none(), "{input}");
    }
    // Deviation: C++ turns these away, as CSS Color 4 does. The csscolorparser
    // crate reads commas, an alpha without its slash, a number that ends in a
    // dot and a space before the parenthesis.
    for input in [
        "oklch(0.6, 0.1, 90)",
        "oklch(0.6 0.1 90, 0.5)",
        "oklch(0.6 0.1 90 0.5)",
        "oklch(1. 0.1 90)",
        "oklch (0.6 0.1 90)",
    ] {
        assert!(Color::parse(input).is_some(), "{input}");
    }
}

// gtest: ColorInterpolate.Oklch
#[test]
fn oklch_interpolate() {
    let blue = Color::blue();
    let green = Color::parse("rgba(0,255,0,.6)").unwrap();
    expect_color(Color::interpolate(&blue, &green, 0.25, ColorSpace::Oklch), 0.0, 0.386_681_5, 0.886_181_3, 0.9);
    expect_color(Color::interpolate(&blue, &green, 0.5, ColorSpace::Oklch), 0.0, 0.532_583_2, 0.661_055_3, 0.8);
    expect_color(Color::interpolate(&blue, &green, 0.75, ColorSpace::Oklch), 0.0, 0.594_188_3, 0.398_122_3, 0.7);
    // The hue takes the short way round, across 0.
    expect_color(
        Color::interpolate(
            &Color::parse("oklch(0.6 0.15 350)").unwrap(),
            &Color::parse("oklch(0.7 0.12 20)").unwrap(),
            0.5,
            ColorSpace::Oklch,
        ),
        0.822_271_2,
        0.409_868,
        0.517_732_5,
        1.0,
    );
}

// gtest: ColorInterpolate.OklchGrayTakesTheOtherHue
#[test]
fn oklch_gray_takes_the_other_hue() {
    let gray = Color::parse("#808080").unwrap();
    let blue = Color::parse("oklch(0.6 0.12 250)").unwrap();
    expect_color(Color::interpolate(&gray, &blue, 0.5, ColorSpace::Oklch), 0.395_584_2, 0.514_941_2, 0.639_869_7, 1.0);
    expect_color(Color::interpolate(&blue, &gray, 0.5, ColorSpace::Oklch), 0.395_584_2, 0.514_941_2, 0.639_869_7, 1.0);
}

// gtest: ColorInterpolate.OklchPremultipliesLightnessAndChroma
#[test]
fn oklch_premultiplies_lightness_and_chroma() {
    expect_color(
        Color::interpolate(
            &Color::parse("oklch(0.7 0.1 30 / 0.2)").unwrap(),
            &Color::parse("oklch(0.5 0.1 250)").unwrap(),
            0.5,
            ColorSpace::Oklch,
        ),
        0.315_826,
        0.208_495_8,
        0.340_319_7,
        0.6,
    );
    expect_color(
        Color::interpolate(&Color::default(), &Color::parse("oklch(0.7 0.1 150)").unwrap(), 0.5, ColorSpace::Oklch),
        0.218_129_4,
        0.345_293,
        0.245_365_1,
        0.5,
    );
    expect_color(Color::interpolate(&Color::default(), &Color::default(), 0.5, ColorSpace::Oklch), 0.0, 0.0, 0.0, 0.0);
}

// Expected values come from GL JS's Color.interpolate (maplibre-gl-style-spec), premultiplied.
// gtest: ColorInterpolate.Lab
#[test]
fn lab() {
    let red = Color::parse("rgba(255,0,0,0.5)").unwrap();
    let blue = Color::blue();
    expect_color(Color::interpolate(&red, &blue, 0.0, ColorSpace::Lab), 0.5, 0.0, 0.0, 0.5);
    expect_color(Color::interpolate(&red, &blue, 0.25, ColorSpace::Lab), 0.556_105, 0.0, 0.194_104_7, 0.625);
    expect_color(Color::interpolate(&red, &blue, 0.5, ColorSpace::Lab), 0.567_614_8, 0.0, 0.400_506_2, 0.75);
    expect_color(Color::interpolate(&red, &blue, 1.0, ColorSpace::Lab), 0.0, 0.0, 1.0, 1.0);
    expect_color(
        Color::interpolate(&Color::black(), &Color::white(), 0.5, ColorSpace::Lab),
        0.466_326_6,
        0.466_326_6,
        0.466_326_6,
        1.0,
    );
}

// gtest: ColorInterpolate.Hcl
#[test]
fn hcl() {
    let red = Color::parse("rgba(255,0,0,0.5)").unwrap();
    expect_color(Color::interpolate(&red, &Color::blue(), 0.5, ColorSpace::Hcl), 0.720_439_5, 0.0, 0.394_412_1, 0.75);
    // A hueless start takes the end's hue.
    expect_color(
        Color::interpolate(&Color::white(), &Color::red(), 0.5, ColorSpace::Hcl),
        1.0,
        0.624_119_8,
        0.503_082_4,
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
        0.282_725_6,
        0.298_255,
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
