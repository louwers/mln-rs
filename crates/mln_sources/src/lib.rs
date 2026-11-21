//! The built-in source types: the style specification's vector, raster,
//! raster-dem, GeoJSON and image sources, and the custom geometry and custom
//! vector sources, whose data the host gives. Each type's module holds its
//! style half (`mln_style`'s `StyleSourceType`), its render half and data
//! origin (the [source type contract](mln_render::source)), and the edits
//! and handles its sources take. They register through
//! [`register_source_types`].
//!
//! With the `runtime` feature, each type's module also has the commands of
//! `mln_runtime`'s maps for its sources, as a trait that the map implements,
//! such as `GeoJsonSourceCommands`, over the runtime's generic source
//! paths.

pub mod custom;
pub mod geojson;
pub mod image;
pub mod tileset;

pub use custom::geometry::CustomGeometrySourceHandle;
pub use custom::vector::CustomVectorSourceHandle;
pub use custom::{CustomGeometrySourceType, CustomVectorSourceType, TileDataFormat};
#[cfg(feature = "runtime")]
pub use custom::{CustomSourceCallbacks, CustomSourceCommands};
#[cfg(feature = "runtime")]
pub use geojson::{ClusterQueries, GeoJsonSourceCommands, GeoJsonSourceSettings};
pub use geojson::{GeoJSONData, GeoJSONOptions, GeoJsonSourceType, geojson_source};
#[cfg(feature = "runtime")]
pub use image::ImageSourceCommands;
pub use image::{ImageSourceType, image_source};
use mln_render::{RegisterError, RegistryBuilder};
pub use tileset::{
    RasterDemSourceType, RasterSourceType, TilesetSourceType, VectorSourceType, raster_dem_source, raster_source,
    vector_source,
};

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

/// The error of a command for the source `id`, which isn't `kind`, as in
/// `source "water" is not a GeoJSON source`.
#[cfg(feature = "runtime")]
fn not_a(id: &str, kind: &str) -> mln_util::Error {
    mln_util::Error::invalid_argument(format!("source \"{id}\" is not {kind}"))
}

#[cfg(test)]
mod tests;
