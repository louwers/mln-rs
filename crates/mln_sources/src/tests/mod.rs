//! Ported gtests of the built-in source types (test/style/source.test.cpp,
//! test/style/conversion/*.test.cpp), one module per C++ file, and the
//! port's own: how their sources convert and describe their data. They
//! register this crate's source types: the facade's registry would hold
//! another copy of them. How their tiles load, the tests of
//! `mln_extension_tests` show through the facade.

mod conversion_geojson_options;
mod conversion_source_options;
mod conversion_tileset;
mod source;

use std::sync::Arc;

use mln_render::RegistryBuilder;
use mln_style::StyleRegistry;

/// What styles parse against: the style half of a registry of the built-in
/// expression functions, the layer types of `mln_layers` and this crate's
/// source types.
pub(crate) fn style_registry() -> Arc<StyleRegistry> {
    let mut builder = RegistryBuilder::new();
    mln_style::expression::functions::register_builtin_functions(builder.functions_mut()).unwrap();
    mln_layers::register_layer_types(&mut builder).unwrap();
    crate::register_source_types(&mut builder).unwrap();
    Arc::clone(builder.build().style())
}

pub(crate) fn parse_json(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("test JSON must parse")
}
