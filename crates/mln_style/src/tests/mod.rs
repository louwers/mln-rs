//! Ported gtests (test/style/**). One module per C++ test file; `#[test]`
//! names are the gtest names in `snake_case`. The tests that need a layer
//! type without a source use `fixture_layer`'s.

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
pub(crate) mod fixture_layer;
mod format_section_override_test;
mod properties;
mod property_expression;
mod source;
mod style;
mod style_image;
mod style_json;
mod style_layer;
mod style_parser;
mod util;
mod variable_anchor_offset_collection;
