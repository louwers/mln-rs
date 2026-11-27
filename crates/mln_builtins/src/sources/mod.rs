//! The style specification's source types: their engine halves, whose style
//! halves are `mln_style::sources`'. Each type's module ([`tileset`],
//! [`geojson`], [`image`]) says how its sources' descriptions load, what
//! they report and what they supply the renderer.

pub mod geojson;
pub mod image;
pub mod tileset;

pub use geojson::{GeoJSONData, GeoJsonSource};
pub use image::ImageSource;
use mln_engine::RegistryBuilder;
use mln_style::expression::functions::RegisterError;
pub use tileset::{RasterDemSource, RasterSource, TilesetSource, VectorSource};

/// Adds the style specification's source types. Fails for a name that an
/// earlier call took.
pub fn register_source_types(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_source_type::<VectorSource>()?;
    builder.add_source_type::<RasterSource>()?;
    builder.add_source_type::<RasterDemSource>()?;
    builder.add_source_type::<GeoJsonSource>()?;
    builder.add_source_type::<ImageSource>()
}
