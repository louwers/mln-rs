//! The tests' own layer and source types: render halves of the style
//! specification's layer types and engine halves of its source types, whose
//! style halves are `mln_style`'s, so that the tests' styles parse as they
//! are. They draw and load as the built-in types do as far as the engine's
//! tests need, and no further: the built-in types' own rules are
//! `mln_builtins`' tests'.

pub(crate) mod geojson;
pub(crate) mod image;
pub(crate) mod layers;
pub(crate) mod tileset;

use mln_style::expression::functions::RegisterError;
use mln_style::sources::tileset::{RasterDemKind, RasterKind, VectorKind};

use crate::RegistryBuilder;

/// Adds the tests' source types.
pub(crate) fn register_source_types(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_source_type::<tileset::TestTileset<VectorKind>>()?;
    builder.add_source_type::<tileset::TestTileset<RasterKind>>()?;
    builder.add_source_type::<tileset::TestTileset<RasterDemKind>>()?;
    builder.add_source_type::<geojson::TestGeoJsonSource>()?;
    builder.add_source_type::<image::TestImageSource>()
}
