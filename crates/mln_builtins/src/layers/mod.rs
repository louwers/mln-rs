//! The style specification's layer types: their render halves, whose style
//! halves are `mln_style::layers`', through the engine's layer contract
//! (`mln_engine::layer`). Each type's module holds its render layer, its
//! programs, their blocks and shaders, and its layout, bucket and meshes;
//! symbol's holds its participant in placement too, through the engine's
//! placement contract (`mln_engine::placement`).

pub mod background;
pub mod circle;
pub mod color_relief;
pub mod fill;
pub mod fill_extrusion;
pub mod heatmap;
pub mod hillshade;
pub mod line;
mod pattern;
mod pattern_layout;
pub mod raster;
mod subdivision;
pub mod symbol;
mod tessellator;

use mln_engine::RegistryBuilder;
use mln_style::expression::functions::RegisterError;

/// Adds the style specification's layer types. Fails for a name that an
/// earlier call took.
pub fn register_layer_types(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_layer_type::<fill::layer::RenderFillLayer>()?;
    builder.add_layer_type::<line::layer::RenderLineLayer>()?;
    builder.add_layer_type::<circle::layer::RenderCircleLayer>()?;
    builder.add_layer_type::<raster::layer::RenderRasterLayer>()?;
    builder.add_layer_type::<background::layer::RenderBackgroundLayer>()?;
    builder.add_layer_type::<hillshade::layer::RenderHillshadeLayer>()?;
    builder.add_layer_type::<color_relief::layer::RenderColorReliefLayer>()?;
    builder.add_layer_type::<fill_extrusion::layer::RenderFillExtrusionLayer>()?;
    builder.add_layer_type::<heatmap::layer::RenderHeatmapLayer>()?;
    builder.add_placing_layer_type::<symbol::placement::SymbolPlacement>()
}

#[cfg(test)]
mod tests;
