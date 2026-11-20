//! The registrations of the built-in layer types and source types, which
//! the facade crate `mln_rs` composes with the built-in expression functions.

use mln_style::expression::functions::RegisterError;
use mln_style::layers::{
    Background, Circle, ColorRelief, Custom, Fill, FillExtrusion, Heatmap, Hillshade, Line, Raster, Symbol,
};
use mln_style::sources::{
    CustomGeometrySourceType, CustomVectorSourceType, GeoJsonSourceType, ImageSourceType, RasterDemSourceType,
    RasterSourceType, VectorSourceType,
};

use crate::RegistryBuilder;

/// Adds the style specification's layer types and the custom layer type.
/// Fails for a name that an earlier call took.
pub fn register_layer_types(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_layer_type::<Fill>()?;
    builder.add_layer_type::<Line>()?;
    builder.add_layer_type::<Circle>()?;
    builder.add_layer_type::<Symbol>()?;
    builder.add_layer_type::<Raster>()?;
    builder.add_layer_type::<Background>()?;
    builder.add_layer_type::<Hillshade>()?;
    builder.add_layer_type::<ColorRelief>()?;
    builder.add_layer_type::<FillExtrusion>()?;
    builder.add_layer_type::<Heatmap>()?;
    builder.add_layer_type::<Custom>()
}

/// Adds the style specification's source types and the custom source types.
/// Fails for a name that an earlier call took.
pub fn register_source_types(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_source_type::<VectorSourceType>()?;
    builder.add_source_type::<RasterSourceType>()?;
    builder.add_source_type::<RasterDemSourceType>()?;
    builder.add_source_type::<GeoJsonSourceType>()?;
    builder.add_source_type::<ImageSourceType>()?;
    builder.add_source_type::<CustomGeometrySourceType>()?;
    builder.add_source_type::<CustomVectorSourceType>()
}
