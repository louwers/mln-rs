use mln_conversion::Convertible;
use mln_util::Padding;

use super::util::parse_json;
use crate::conversion::{Converter, Error};

// gtest: StyleConversion.Padding
#[test]
fn padding() {
    let mut error = Error::default();
    let parse_padding = |src: &str, error: &mut Error| {
        let value = parse_json(src);
        <Padding as Converter>::convert(&Convertible::from(&value), error)
    };

    {
        // Single value is applied to all sides
        let padding = parse_padding("3", &mut error).expect("padding");
        assert_eq!(padding.top, 3.0);
        assert_eq!(padding.right, 3.0);
        assert_eq!(padding.bottom, 3.0);
        assert_eq!(padding.left, 3.0);
    }
    {
        // Two values apply to [top/bottom, left/right]
        let padding = parse_padding("[3.5, 4.5]", &mut error).expect("padding");
        assert_eq!(padding.top, 3.5);
        assert_eq!(padding.bottom, 3.5);
        assert_eq!(padding.left, 4.5);
        assert_eq!(padding.right, 4.5);
    }
    {
        // Three values apply to [top, left/right, bottom] e.g. [2, 3, 1];
        let padding = parse_padding("[2, 3, 1]", &mut error).expect("padding");
        assert_eq!(padding.top, 2.0);
        assert_eq!(padding.left, 3.0);
        assert_eq!(padding.right, 3.0);
        assert_eq!(padding.bottom, 1.0);
    }
    {
        // Four values apply to [top, right, bottom, left], e.g. [2, 3, 1, 0].
        let padding = parse_padding("[-2.5, -3.5, -1.0, -0.5]", &mut error).expect("padding");
        assert_eq!(padding.top, -2.5);
        assert_eq!(padding.right, -3.5);
        assert_eq!(padding.bottom, -1.0);
        assert_eq!(padding.left, -0.5);
    }

    // Invalid values

    let expected_error = "value must be a number or an array of numbers (between 1 and 4 elements)";
    for src in [
        "null",            // null
        "false",           // bool
        "[]",              // empty array
        "[1, 2, 3, 4, 5]", // too long array
        "[true, false]",   // array of wrong types
        "[null]",          // array with null
        "{\"foo\": 1}",    // object
    ] {
        let padding = parse_padding(src, &mut error);
        assert!(padding.is_none(), "{src}");
        assert_eq!(error.message, expected_error, "{src}");
    }
}
