// The style spec's parse_css_color.test.ts, in its order, each test named
// after its describe block and title. The spec's "should parse valid hsl
// values" mocks hslToRgb to see the parsed hue, saturation and lightness;
// here it converts the expected ones with `hsl_to_srgb` instead.

use super::*;

/// The tolerance of `toEqual`, which compares JS doubles that Rust's f64
/// arithmetic matches all but exactly.
const EXACT: f64 = 1e-9;

/// The tolerance of the spec's `expectCloseToArray`, 5 digits.
const CLOSE: f64 = 5e-6;

/// The tolerance of vitest's `expect.closeTo`, 2 digits.
const CLOSE_TO: f64 = 5e-3;

fn rgba(text: &str) -> Option<[f64; 4]> {
    parse(text).map(|([r, g, b], alpha)| [r, g, b, alpha])
}

fn within(actual: [f64; 4], expected: [f64; 4], tolerance: f64) -> bool {
    actual.iter().zip(expected).all(|(actual, expected)| (actual - expected).abs() <= tolerance)
}

#[track_caller]
fn assert_rgba_within(text: &str, expected: [f64; 4], tolerance: f64) {
    let actual = rgba(text);
    assert!(
        actual.is_some_and(|actual| within(actual, expected, tolerance)),
        "{text}: expected {expected:?}, got {actual:?}"
    );
}

#[track_caller]
fn assert_rgba(text: &str, expected: [f64; 4]) {
    assert_rgba_within(text, expected, EXACT);
}

#[track_caller]
fn assert_rgba_close(text: &str, expected: [f64; 4]) {
    assert_rgba_within(text, expected, CLOSE);
}

/// `text` and `other` both parse, to the same color within `tolerance`.
#[track_caller]
fn assert_same_within(text: &str, other: &str, tolerance: f64) {
    let (actual, expected) = (rgba(text), rgba(other));
    assert!(
        actual.zip(expected).is_some_and(|(actual, expected)| within(actual, expected, tolerance)),
        "{text}: expected {other} ({expected:?}), got {actual:?}"
    );
}

#[track_caller]
fn assert_same(text: &str, other: &str) {
    assert_same_within(text, other, EXACT);
}

#[track_caller]
fn assert_invalid(text: &str) {
    assert!(parse(text).is_none(), "{text}: expected None, got {:?}", rgba(text));
}

#[track_caller]
fn alpha(text: &str) -> f64 {
    rgba(text).unwrap_or_else(|| panic!("{text}: expected a color, got None"))[3]
}

/// The color of `hsl()` with these hue, saturation, lightness and alpha.
fn hsl(hue: f64, saturation: f64, lightness: f64, alpha: f64) -> [f64; 4] {
    let [r, g, b] = hsl_to_srgb(hue, saturation, lightness);
    [r, g, b, alpha]
}

#[test]
fn named_colors_are_sorted() {
    assert!(NAMED_COLORS.is_sorted_by_key(|&(name, _)| name));
}

#[test]
fn color_keywords_should_parse_valid_color_names() {
    assert_rgba("white", [1.0, 1.0, 1.0, 1.0]);
    assert_rgba("black", [0.0, 0.0, 0.0, 1.0]);
    assert_rgba("RED", [1.0, 0.0, 0.0, 1.0]);
    assert_rgba("AquaMarine", [127.0 / 255.0, 255.0 / 255.0, 212.0 / 255.0, 1.0]);
    assert_rgba("steelblue", [70.0 / 255.0, 130.0 / 255.0, 180.0 / 255.0, 1.0]);
    assert_rgba("rebeccapurple", [0.4, 0.2, 0.6, 1.0]);
}

#[test]
fn color_keywords_should_parse_transparent_keyword_as_transparent_black() {
    assert_rgba("transparent", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("Transparent", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("TRANSPARENT", [0.0, 0.0, 0.0, 0.0]);
}

#[test]
fn color_keywords_should_return_undefined_when_provided_with_invalid_color_name() {
    for text in ["not a color name", "", "blak", "aqua-marine", "aqua_marine", "aqua marine", "__proto__", "valueOf"] {
        assert_invalid(text);
    }
}

#[test]
fn rgb_hex_should_parse_valid_rgb_hex_values() {
    // hex 3
    assert_rgba("#fff", [1.0, 1.0, 1.0, 1.0]);
    assert_rgba("#000", [0.0, 0.0, 0.0, 1.0]);
    assert_rgba("#369", [0.2, 0.4, 0.6, 1.0]);

    // hex 4
    assert_rgba("#ffff", [1.0, 1.0, 1.0, 1.0]);
    assert_rgba("#fff0", [1.0, 1.0, 1.0, 0.0]);
    assert_rgba("#0000", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("#FFFC", [1.0, 1.0, 1.0, 0.8]);
    assert_rgba("#234a", [34.0 / 255.0, 51.0 / 255.0, 68.0 / 255.0, 2.0 / 3.0]);

    // hex 6
    assert_rgba("#ffffff", [1.0, 1.0, 1.0, 1.0]);
    assert_rgba("#000000", [0.0, 0.0, 0.0, 1.0]);
    assert_rgba("#008000", [0.0, 128.0 / 255.0, 0.0, 1.0]);
    assert_rgba("#b96710", [185.0 / 255.0, 103.0 / 255.0, 16.0 / 255.0, 1.0]);

    // hex 8
    assert_rgba("#ffffffff", [1.0, 1.0, 1.0, 1.0]);
    assert_rgba("#000000ff", [0.0, 0.0, 0.0, 1.0]);
    assert_rgba("#00000000", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("#FFCc9933", [255.0 / 255.0, 204.0 / 255.0, 153.0 / 255.0, 0.2]);
    assert_rgba("#4682B466", [70.0 / 255.0, 130.0 / 255.0, 180.0 / 255.0, 0.4]);
}

#[test]
fn rgb_hex_should_return_undefined_when_provided_with_invalid_rgb_hex_value() {
    for text in
        ["#", "#f", "#ff", "#ffg", "#fffg", "#fffff", "#fffffg", "#fffffff", "#fffffffg", "#fffffffff", "fff", "# fff"]
    {
        assert_invalid(text);
    }
}

#[test]
fn rgb_functions_should_parse_valid_rgb_values() {
    // rgb 0..255
    assert_rgba("rgb(0 51 0)", [0.0, 0.2, 0.0, 1.0]);
    for other in [
        "rgb(0, 51, 0)",
        "rgb(0.0, 51.0, +0.0)",
        "rgba(0, 51, 0)",
        "rgba(0, 51, 0, 1)",
        "rgba(0, 51, 0, 100%)",
        "rgba( 0, 51, 0, 100% )",
        "rgba( 00 ,51 ,0 ,100% )",
        " rgb(.0 51 0 / 1)",
        "rgb(0.0 51.0 0.0 / 1.0) ",
        "rgb(0 51 0 / 1.0)",
        "RGB(0 51 0 / 100%)",
        "rgb(  0  51  0/1  )",
        "rgb(0 5.1e+1 0 / .1e1)",
    ] {
        assert_same("rgb(0 51 0)", other);
    }
    assert_rgba("rgb(0,0.5,1)", [0.0, 0.5 / 255.0, 1.0 / 255.0, 1.0]);
    assert_same("rgb(0,0.5,1)", "rgb(0 0.5 1)");
    assert_rgba("rgb(0,0,0,.1e-4)", [0.0, 0.0, 0.0, 1e-5]);
    assert_rgba("rgb(102,51,153)", [0.4, 0.2, 0.6, 1.0]);
    assert_rgba("rgb(26,207,26,0.5)", [26.0 / 255.0, 207.0 / 255.0, 26.0 / 255.0, 0.5]);
    assert_rgba("rgba(26,207,26,.73)", [26.0 / 255.0, 207.0 / 255.0, 26.0 / 255.0, 0.73]);
    assert_rgba("rgb(127.5 0 0)", [0.5, 0.0, 0.0, 1.0]);
    assert_rgba("rgb(128 0 0)", [128.0 / 255.0, 0.0, 0.0, 1.0]);
    assert_rgba("rgb(100 200 300)", [100.0 / 255.0, 200.0 / 255.0, 1.0, 1.0]);
    assert_rgba("rgb(-0 255 153)", [0.0, 1.0, 0.6, 1.0]);
    assert_rgba("rgb(-100 300 153)", [0.0, 1.0, 0.6, 1.0]);
    assert_rgba("rgb(-51, 306, 0)", [0.0, 1.0, 0.0, 1.0]);
    assert_rgba("rgba(0,0,0,0.1)", [0.0, 0.0, 0.0, 0.1]);
    assert_same("rgba(0,0,0,0.1)", "rgb(0 0 0 / .1)");
    assert_same("rgba(0,0,0,0.1)", "rgb(0 0 0 / 10%)");
    assert_rgba("rgb(0 0 0 / .0)", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("rgb(0 0 0 / -.0)", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("rgb(0 0 0 / -3.4e-2)", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("rgb(0 0 0 / -.2)", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("rgb(0 0 0 / -10%)", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("rgb(0 0 0 / 1.0)", [0.0, 0.0, 0.0, 1.0]);
    assert_rgba("rgb(0 0 0 / 1.1)", [0.0, 0.0, 0.0, 1.0]);
    assert_rgba("rgb(0 0 0 / 110%)", [0.0, 0.0, 0.0, 1.0]);

    // rgb 0%..100%
    assert_rgba("rgb(0% 50% 0%)", [0.0, 0.5, 0.0, 1.0]);
    for other in [
        "rgb(0%, 50%, 0%)",
        "rgba(0%, 50%, 0%, 1)",
        "rgba(0%, 50%, 0%, 100%)",
        "rgba(-0e1%,5E1%,0%,1)",
        "rgb(0% 50% 0% / 1.0)",
        "rgb(.0% 50% 0% / 100%)",
        "rgb(0.0% 50.0% 0.0% / 1)",
        "rgb( 0% 50% 0% /  100% )",
        "rgb(-1e-9% 50% 0% /1)",
    ] {
        assert_same("rgb(0% 50% 0%)", other);
    }
    assert_rgba("rgb(-0% 100% 60%)", [0.0, 1.0, 0.6, 1.0]);
    assert_rgba("rgb(-10% 200% 60%)", [0.0, 1.0, 0.6, 1.0]);
    assert_rgba("rgb(100%,200%,300%)", [1.0, 1.0, 1.0, 1.0]);
    assert_rgba("rgb(128% 51% 255%)", [1.0, 0.51, 1.0, 1.0]);
    assert_rgba("rgba(0%,0%,0%,0.1)", [0.0, 0.0, 0.0, 0.1]);
    assert_same("rgba(0%,0%,0%,0.1)", "rgb(0% 0% 0% / .1)");
    assert_same("rgba(0%,0%,0%,0.1)", "rgb(0% 0% 0% / 10%)");
    assert_rgba("rgb(0% 0% 0% / .0)", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("rgb(0% 0% 0% / -.0)", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("rgb(0% 0% 0% / -3.4e-2)", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("rgb(0% 0% 0% / -.2)", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("rgb(0% 0% 0% / -10%)", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("rgb(0% 0% 0% / 1.0)", [0.0, 0.0, 0.0, 1.0]);
    assert_rgba("rgb(0% 0% 0% / 1.1)", [0.0, 0.0, 0.0, 1.0]);
    assert_rgba("rgb(0% 0% 0% / 110%)", [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn rgb_functions_should_return_undefined_when_provided_with_invalid_rgb_value() {
    assert_invalid("rgb (0,0,0)");
    assert_invalid("rgba (0,0,0,0)");

    let invalid_arguments = [
        // values must be all numbers or all percentages
        "10%, 50%, 0",
        "255, 50%, 0%",
        "10%, 50%, 0, 1",
        "255, 50%, 0%, 1",
        "0 50% 255 / 1",
        "0 50% 0 / 1",
        "128 51% 255",
        // comma optional syntax requires no commas at all
        "0, 0 0",
        "0, 0, 0 0",
        "0, 0, 0 / 1",
        "0 0 0, 1",
        // angles are not accepted in the rgb function
        "0, 0, 0deg",
        "0, 0, 0, 0deg",
        // keywords are not accepted in the rgb function
        "0, 0, light",
        "0, 0, 0, light",
        // invalid numbers
        "--1,0,0",
        "+-1,0,0",
        "++1,0,0",
        "1.1.1,0,0",
        ".-1,0,0",
        "..1,0,0",
        "1e1.1,0,0",
        "1e.1,0,0",
        "--1e1,0,0",
        "+-1e1,0,0",
        // the rgb function requires 3 or 4 arguments
        "",
        "0",
        ", 0,",
        "0, 0",
        "0, 0,",
        ", 0, 0",
        "0 0 0 /",
        "0, 0, 0, 0, 0",
        "0, 0, 0, 0, 0,",
        ", 0, 0, 0, 0, 0",
        "0%",
        ", 0%,",
        "0%, 0%",
        "0%, 0%,",
        ", 0%, 0%",
        "0%, 0%, 0%,",
        "0% 0% 0% /",
        "0%, 0%, 0%, 0%, 0%",
        "0%, 50%, 100%,",
        ", 0%, 50%, 100%",
        ", 0%, 50%, 100%, 100%",
        "0%, 50%, 100%, 100%,",
    ];
    for arguments in invalid_arguments {
        for function in ["rgb", "rgba"] {
            assert_invalid(&format!("{function}({arguments})"));
        }
    }
}

#[test]
fn hsl_functions_should_parse_valid_hsl_values() {
    assert_rgba("hsl(300,100%,25.1%)", hsl(300.0, 100.0, 25.1, 1.0));
    for other in [
        "hsla(300,100%,25.1%,1)",
        "hsla(300,100%,25.1%,100%)",
        "hsl(300 100% 25.1%)",
        "hsl(300 100% 25.1%/1.0)",
        "hsl(300.0 100% 25.1% / 100%)",
        "hsl(300deg 100% 25.1% / 100%)",
    ] {
        assert_same("hsl(300,100%,25.1%)", other);
    }

    assert_rgba("hsl(240,0%,55%,0.2)", hsl(240.0, 0.0, 55.0, 0.2));
    for other in [
        "hsla(240.0,0%,55%,0.2)",
        "hsla( 240 ,.0% ,55.0% ,20% )",
        "hsl(240 0% 55% / 0.2)",
        "hsl(240 0% 55% / 20%)",
        "hsl(24e1deg 0e1% 55% / 2e-1)",
        "hsla(240 -1e-7% 55% / 2e1%)",
    ] {
        assert_same("hsl(240,0%,55%,0.2)", other);
    }

    assert_rgba("hsl(240,0%,55%,0.9)", hsl(240.0, 0.0, 55.0, 0.9));
    assert_rgba("hsl(240,0%,55%,.0)", hsl(240.0, 0.0, 55.0, 0.0));
    assert_rgba("hsl(700 0% 67.3% / 100%)", hsl(700.0, 0.0, 67.3, 1.0));
    assert_rgba("Hsl( -100 -10.5% 67.3% / 100% )", hsl(-100.0, 0.0, 67.3, 1.0));
}

#[test]
fn hsl_functions_should_parse_valid_hsl_values_and_convert_to_rgb() {
    assert_rgba("hsl(0 100% 50%)", [1.0, 0.0, 0.0, 1.0]);
    assert_rgba("hsl(240 100% 50%)", [0.0, 0.0, 1.0, 1.0]);
    assert_rgba("hsl(240 100% 25%)", [0.0, 0.0, 0.5, 1.0]);
    let text = "hsl(273 75% 60%)";
    let actual = rgba(text);
    assert!(
        actual.is_some_and(|[r, g, b, alpha]| {
            (r - 0.63).abs() <= CLOSE_TO && (g - 0.3).abs() <= CLOSE_TO && (b - 0.9).abs() <= EXACT && alpha == 1.0
        }),
        "{text}: expected [~0.63, ~0.3, 0.9, 1], got {actual:?}"
    );

    assert_rgba("hsl(0 0% 0%)", [0.0, 0.0, 0.0, 1.0]);
    assert_rgba("hsl(0 0% 0% / 0)", [0.0, 0.0, 0.0, 0.0]);
    for other in [
        "hsl(0,0%,0%,+0)",
        "hsla(0deg,0%,0%,-0)",
        "hsla(0,0%,0%,0%)",
        " hsla(.0,.0%,.0%,.0%)",
        "hsla(  0 ,0% ,0% ,.0 ) ",
    ] {
        assert_same("hsl(0 0% 0% / 0)", other);
    }

    assert_rgba("hsl(120 100% 25%)", [0.0, 0.5, 0.0, 1.0]);
    for other in [
        "hsl(120.0 100.0% 25.0%)",
        "hsl(120deg 100% 25%)",
        "hsl(120 100% 25% / 1.0)",
        "hsl(120deg 100% 25% / 1)",
        "hsl(120 100% 25% / 100%)",
        "hsl(120,100%,25%,100%)",
        "hsla(120deg,100%,25%,100%)",
    ] {
        assert_same("hsl(120 100% 25%)", other);
    }

    assert_rgba("hsl(120 100% 50% / .25)", [0.0, 1.0, 0.0, 0.25]);
    for other in [
        "HSLA(120,100%,50%,.25)",
        "hsla(120,100%,50%,25%)",
        "hsl(120 100% 50%/.25)",
        "hsl(120deg 100% 50% / 25%)",
        "hsl(480 100% 50% / 25%)",
        "hsl(-240deg 100% 50% / 25%)",
    ] {
        assert_same("hsl(120 100% 50% / .25)", other);
    }

    assert_same("hsl(0.0 200% 50%)", "hsl(0 100% 50%)");
    assert_same("hsl(-0 -100% -100%)", "hsl(0 0% 0%)");
    assert_rgba("hsl(0 0% 0% / .0)", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("hsl(0 0% 0% / -.0)", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("hsl(0 0% 0% / -3.4e-2)", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("hsl(0 0% 0% / -.2)", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("hsl(0 0% 0% / -10%)", [0.0, 0.0, 0.0, 0.0]);
    assert_rgba("hsl(0 0% 0% / 1.0)", [0.0, 0.0, 0.0, 1.0]);
    assert_rgba("hsl(0 0% 0% / 1.1)", [0.0, 0.0, 0.0, 1.0]);
    assert_rgba("hsl(0 0% 0% / 110%)", [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn hsl_functions_should_return_undefined_when_provided_with_invalid_hsl_value() {
    assert_invalid("hsl (0,0%,0%)");
    assert_invalid("hsla (0,0%,0%,1)");

    let invalid_arguments = [
        "0,0%,0 %",
        // the first parameter of hsl/hsla must be a number or angle
        "0%,0%,0%",
        "0 deg,0%,0%",
        // the second and third parameters of hsl/hsla must be a percent
        "10, 50%, 0",
        // comma optional syntax requires no commas at all
        "0, 0% 0%",
        "0, 0% 0%, 1",
        // keywords are not accepted in the hsl function
        "0,0%,light,1",
        // invalid numbers
        "--1,0%,0%",
        "+-1,0%,0%",
        "++1,0%,0%",
        "1.1.1,0%,0%",
        ".-1,0%,0%",
        "..1,0%,0%",
        "1e1.1,0%,0%",
        "1e.1,0%,0%",
        "--1e1,0%,0%",
        "+-1e1,0%,0%",
        // the hsl function requires 3 or 4 arguments
        "",
        "0",
        "0 0%",
        "0, 0%,",
        ", 0%, 0%",
        "0,0%,0%,1,0%",
        "0,0,0",
        "0,0%,0",
        "0,0,0%",
        "0 0% 0% /",
        "0,0%,0%,",
        ", 0%,0%,0%",
    ];
    for arguments in invalid_arguments {
        for function in ["hsl", "hsla"] {
            assert_invalid(&format!("{function}({arguments})"));
        }
    }
}

#[test]
fn oklch_function_should_parse_valid_oklch_values_and_convert_to_rgb() {
    assert_rgba_close("oklch(70% 0.1 30)", [0.836_736, 0.526_627, 0.475_198, 1.0]);
    assert_rgba_close("oklch(0.8 0.05 160 / 0.75)", [0.638_114, 0.784_944, 0.701_091, 0.75]);

    let yellowish = [0.593_719, 0.490_828, 0.190_116, 1.0];
    for text in [
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
        assert_rgba_close(text, yellowish);
    }
}

#[test]
fn oklch_function_should_wrap_a_huge_angle_before_converting_it_to_degrees() {
    assert_same("oklch(0.6 0.1 1e306turn)", "oklch(0.6 0.1 0)");
    assert!(parse("oklch(0.6 0.1 1e307rad)").is_some());
}

#[test]
fn oklch_function_should_clamp_lightness_chroma_and_alpha() {
    assert_same("oklch(1.2 0.1 30)", "oklch(1 0.1 30)");
    assert_same("oklch(120% 0.1 30)", "oklch(1 0.1 30)");
    assert_same("oklch(-0.1 0.1 30)", "oklch(0 0.1 30)");
    assert_same("oklch(0.6 -0.1 30)", "oklch(0.6 0 30)");
    assert_same("oklch(0.6 0.1 30 / 1.5)", "oklch(0.6 0.1 30)");
    assert_eq!(alpha("oklch(0.6 0.1 30 / -1)"), 0.0);
    assert_eq!(alpha("oklch(0.6 0.1 30 / 50%)"), 0.5);
}

#[test]
fn oklch_function_should_read_none_as_zero() {
    assert_same("oklch(none 0.1 90)", "oklch(0 0.1 90)");
    assert_rgba_close("oklch(0.6 none 90)", [0.502_111, 0.502_111, 0.502_111, 1.0]);
    assert_rgba_close("oklch(0.6 0.1 none)", [0.692_324, 0.398_84, 0.492_791, 1.0]);
    assert_eq!(alpha("oklch(0.6 0.1 90 / none)"), 0.0);
}

#[test]
fn oklch_function_should_keep_a_color_outside_srgb_outside_0_1() {
    assert_rgba_close("oklch(0.7 0.3 140)", [-0.240_335, 0.771_585, -0.305_472, 1.0]);
    assert_rgba_close("oklch(0.9 0.3 300)", [1.004_286, 0.625_509, 1.515_87, 1.0]);
}

#[test]
fn oklch_function_should_return_undefined_when_provided_with_invalid_oklch_value() {
    assert_invalid("oklch (0.6 0.1 90)");
    assert_invalid("oklch(0.6 0.1 90");
    assert_invalid("oklch(0.6 0.1 90))");

    let invalid_arguments = [
        // commas are not allowed
        "0.6, 0.1, 90",
        "0.6 0.1 90, 0.5",
        // oklch requires 3 arguments and an optional alpha after a slash
        "",
        "0.6 0.1",
        "0.6 0.1 90 0.5",
        "0.6 0.1 90 /",
        "0.6 0.1 90 / 0.5 / 0.5",
        // the hue is a number or an angle
        "0.6 0.1 90%",
        "0.6 0.1 90 deg",
        "0.6 0.1 90grads",
        "0.6 0.1 nonedeg",
        // lightness and chroma are numbers or percentages
        "0.6deg 0.1 90",
        "0.6 0.1deg 90",
        // invalid numbers
        "1. 0.1 90",
        "0.6 0.1 --90",
        "0.6 0.1 9e",
        "0.6 0.1 1e999",
    ];
    for arguments in invalid_arguments {
        assert_invalid(&format!("oklch({arguments})"));
    }
}

#[test]
fn oklab_function_should_parse_valid_oklab_values_and_convert_to_rgb() {
    assert_rgba_close("oklab(70% 0.086603 0.05)", [0.836_736, 0.526_627, 0.475_198, 1.0]);
    assert_rgba_close("oklab(0.6 0 0 / 0.75)", [0.502_111, 0.502_111, 0.502_111, 0.75]);

    let yellowish = [0.593_719, 0.490_828, 0.190_116, 1.0];
    for text in [
        "oklab(0.6 0 0.1)",
        "oklab(60% 0% 25%)",
        "oklab(0.6 none 0.1)",
        "OKLAB(60% 0 0.1)",
        "oklab(6e-1 0 1E-1)",
        "oklab(+.6 -0 .1 / 1)",
        " oklab(  0.6\t0\n0.1  /  100% ) ",
    ] {
        assert_rgba_close(text, yellowish);
    }
}

#[test]
fn oklab_function_should_read_a_and_b_as_signed_with_100_percent_as_0_4() {
    assert_same_within("oklab(0.6 -25% 10%)", "oklab(0.6 -0.1 0.04)", CLOSE);
    let (text, other) = ("oklab(0.6 0.1 -0.1)", "oklab(0.6 0.1 0.1)");
    assert!(
        !rgba(text).zip(rgba(other)).is_some_and(|(actual, other)| within(actual, other, EXACT)),
        "{text}: expected a color other than {other}'s"
    );
}

#[test]
fn oklab_function_should_clamp_lightness_and_alpha() {
    assert_same("oklab(1.2 0.1 0.1)", "oklab(1 0.1 0.1)");
    assert_same("oklab(120% 0.1 0.1)", "oklab(1 0.1 0.1)");
    assert_same("oklab(-0.1 0.1 0.1)", "oklab(0 0.1 0.1)");
    assert_same("oklab(0.6 0.1 0.1 / 1.5)", "oklab(0.6 0.1 0.1)");
    assert_eq!(alpha("oklab(0.6 0.1 0.1 / -1)"), 0.0);
    assert_eq!(alpha("oklab(0.6 0.1 0.1 / 50%)"), 0.5);
}

#[test]
fn oklab_function_should_read_none_as_zero() {
    assert_same("oklab(none 0.1 0.1)", "oklab(0 0.1 0.1)");
    assert_rgba_close("oklab(0.6 none none)", [0.502_111, 0.502_111, 0.502_111, 1.0]);
    assert_eq!(alpha("oklab(0.6 0.1 0.1 / none)"), 0.0);
}

#[test]
fn oklab_function_should_keep_a_color_outside_srgb_outside_0_1() {
    assert_rgba_close("oklab(0.7 -0.229813 0.192836)", [-0.240_334, 0.771_585, -0.305_471, 1.0]);
}

#[test]
fn oklab_function_should_return_undefined_when_provided_with_invalid_oklab_value() {
    assert_invalid("oklab (0.6 0 0.1)");
    assert_invalid("oklab(0.6 0 0.1");
    assert_invalid("oklab(0.6 0 0.1))");

    let invalid_arguments = [
        // commas are not allowed
        "0.6, 0, 0.1",
        "0.6 0 0.1, 0.5",
        // oklab requires 3 arguments and an optional alpha after a slash
        "",
        "0.6 0",
        "0.6 0 0.1 0.5",
        "0.6 0 0.1 /",
        "0.6 0 0.1 / 0.5 / 0.5",
        // every component is a number or a percentage
        "0.6deg 0 0.1",
        "0.6 0deg 0.1",
        "0.6 0 0.1turn",
        // invalid numbers
        "1. 0 0.1",
        "0.6 0 --0.1",
        "0.6 0 9e",
        "0.6 0 1e999",
    ];
    for arguments in invalid_arguments {
        assert_invalid(&format!("oklab({arguments})"));
    }
}

#[test]
fn display_p3_function_should_parse_valid_display_p3_values_and_convert_to_rgb() {
    assert_rgba_close("color(display-p3 0.917488 0.200287 0.138561)", [1.0, 0.0, 0.0, 1.0]);
    assert_rgba_close("color(display-p3 0.5 0.5 0.5 / 0.75)", [0.5, 0.5, 0.5, 0.75]);

    let tan = [0.836_748, 0.589_605, 0.363_997, 1.0];
    for text in [
        "color(display-p3 0.8 0.6 0.4)",
        "color(display-p3 80% 60% 40%)",
        "COLOR(DISPLAY-P3 0.8 0.6 0.4)",
        "color(display-p3 8e-1 6E-1 +.4)",
        "color(display-p3 0.8 0.6 0.4 / 1)",
        " color(  display-p3\t0.8\n0.6 0.4  /  100% ) ",
    ] {
        assert_rgba_close(text, tan);
    }
}

#[test]
fn display_p3_function_should_clamp_alpha() {
    assert_same("color(display-p3 0.8 0.6 0.4 / 1.5)", "color(display-p3 0.8 0.6 0.4)");
    assert_eq!(alpha("color(display-p3 0.8 0.6 0.4 / -1)"), 0.0);
    assert_eq!(alpha("color(display-p3 0.8 0.6 0.4 / 50%)"), 0.5);
}

#[test]
fn display_p3_function_should_read_none_as_zero() {
    assert_same("color(display-p3 none 0.6 0.4)", "color(display-p3 0 0.6 0.4)");
    assert_eq!(alpha("color(display-p3 0.8 0.6 0.4 / none)"), 0.0);
}

#[test]
fn display_p3_function_should_keep_a_color_outside_srgb_outside_0_1() {
    assert_rgba_close("color(display-p3 0 1 0)", [-0.511_605, 1.018_266, -0.310_675, 1.0]);
    assert_rgba_close("color(display-p3 1.2 -0.1 0.3)", [1.3114, -0.302_01, 0.251_398, 1.0]);
}

#[test]
fn display_p3_function_should_give_a_huge_component_the_color_of_component_1e6() {
    assert_same("color(display-p3 1e200 0 0)", "color(display-p3 1e6 0 0)");
    assert_same("color(display-p3 -1e200 0 0)", "color(display-p3 -1e6 0 0)");
    for text in ["color(display-p3 1e200 0 0)", "color(display-p3 -1e200 0 0)"] {
        let actual = rgba(text);
        assert!(
            actual.is_some_and(|actual| actual.iter().all(|c| c.is_finite())),
            "{text}: expected finite channels, got {actual:?}"
        );
    }
}

#[test]
fn display_p3_function_should_return_undefined_when_provided_with_invalid_display_p3_value() {
    assert_invalid("color (display-p3 0.8 0.6 0.4)");
    assert_invalid("color(display-p3 0.8 0.6 0.4");
    assert_invalid("color(display-p3 0.8 0.6 0.4))");

    let invalid_arguments = [
        // commas are not allowed
        "display-p3 0.8, 0.6, 0.4",
        // display-p3 requires 3 arguments and an optional alpha after a slash
        "display-p3",
        "display-p3 0.8 0.6",
        "display-p3 0.8 0.6 0.4 0.5",
        "display-p3 0.8 0.6 0.4 /",
        "display-p3 0.8 0.6 0.4 / 0.5 / 0.5",
        // display-p3 is the only supported color space
        "0.8 0.6 0.4",
        "srgb 0.8 0.6 0.4",
        "rec2020 0.8 0.6 0.4",
        "displayp3 0.8 0.6 0.4",
        // every component is a number or a percentage
        "display-p3 0.8deg 0.6 0.4",
        // invalid numbers
        "display-p3 1. 0.6 0.4",
        "display-p3 0.8 --0.6 0.4",
        "display-p3 0.8 0.6 1e999",
    ];
    for arguments in invalid_arguments {
        assert_invalid(&format!("color({arguments})"));
    }
}

// Deviation: an infinite hue is no angle, so hsl() turns it away rather than
// making NaN channels of it, as the spec does.
#[test]
fn hsl_turns_away_an_infinite_hue() {
    assert_invalid("hsl(1e999 50% 50%)");
    assert_invalid("hsl(-1e999deg, 50%, 50%)");
}

#[test]
fn hex_turns_away_a_sign() {
    assert_invalid("#+f00ff");
    assert_invalid("#00+f00");
}
