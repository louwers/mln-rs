//! Ported gtests (test/style/**) plus the M1 render-test style check.
//! One module per C++ test file; `#[test]` names are the gtest names in
//! `snake_case`.

mod conversion_function;
mod conversion_geojson_options;
mod conversion_layer;
mod conversion_light;
mod conversion_padding;
mod conversion_projection;
mod conversion_property_value;
mod conversion_source_options;
mod conversion_tileset;
mod diff;
mod expression_global_state;
mod filter;
mod format_section_override_test;
mod lat_lng_property;
mod layer_round_trip;
mod layout_difference;
mod m1_render_test_styles;
mod position;
mod properties;
mod property_expression;
mod rotation;
mod source;
mod style;
mod style_image;
mod style_json;
mod style_layer;
mod style_parser;
pub(crate) mod util;
mod util_gtest;
mod variable_anchor_offset_collection;

/// gtest `EXPECT_NEAR(val1, val2, abs_error)` (in double).
#[track_caller]
fn assert_near(val1: f64, val2: f64, abs_error: f64) {
    let diff = (val1 - val2).abs();
    assert!(diff <= abs_error, "NEAR: |{val1:?} - {val2:?}| = {diff:?} > {abs_error:?}");
}
