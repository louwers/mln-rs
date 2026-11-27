//! `layer_properties!`'s tables, against the ones that style-codegen writes
//! for the spec's types.

use crate::layers::line_layer_properties;

/// The line layer type's properties, declared as a crate outside the
/// style declares its own.
#[expect(dead_code, unreachable_pub, reason = "the test reads only the table")]
mod line {
    crate::layer_properties!(
        pub mod paint: paint {
            line_blur: data_driven f32 = 0.0_f32,
            line_color: data_driven Color = Color::black(),
            line_dasharray: cross_faded_or_data_driven Vec<f32> = Vec::new(),
            line_gap_width: data_driven f32 = 0.0_f32,
            line_gradient: color_ramp Color = Color::default(),
            line_layer_blend: constant LayerBlendType = LayerBlendType::Normal,
            line_layer_opacity: constant f32 = 1.0_f32,
            line_offset: data_driven f32 = 0.0_f32,
            line_opacity: data_driven f32 = 1.0_f32,
            line_pattern: cross_faded_data_driven Image = Image::default(),
            line_translate: constant [f32; 2] = [0.0_f32, 0.0_f32],
            line_translate_anchor: constant TranslateAnchorType = TranslateAnchorType::Map,
            line_width: data_driven f32 = 1.0_f32,
        }
        pub mod layout: layout {
            line_cap: data_driven LineCapType = LineCapType::Butt,
            line_join: data_driven LineJoinType = LineJoinType::Miter,
            line_miter_limit: data_driven f32 = 2.0_f32,
            line_round_limit: data_driven f32 = 1.0_f32,
            line_sort_key: data_driven f32 = 0.0_f32,
        }
        use crate::color::Color, crate::expression::Image, crate::types::LayerBlendType,
            crate::types::TranslateAnchorType, crate::types::LineCapType, crate::types::LineJoinType;
    );
}

#[test]
fn a_declared_table_is_the_one_codegen_writes() {
    assert_eq!(format!("{:?}", line_layer_properties::PROPERTIES), format!("{:?}", line::PROPERTIES));
}
