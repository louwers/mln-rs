//! The built-in source types.
//!
//! Each type's module ([`tileset`], [`geojson`], [`image`], [`custom`]) is
//! canonical for its marker type, options and data, and makes its sources.
//! The custom sources' modules hold the handles that their hosts answer
//! tile requests through, and re-export the types their callers name.

pub mod custom;
pub mod custom_geometry_source;
pub mod custom_vector_source;
pub mod geojson;
pub mod image;
pub mod tileset;

pub use custom::{CustomGeometrySourceType, CustomVectorSourceType, TileDataFormat};
pub use custom_geometry_source::CustomGeometrySourceHandle;
pub use custom_vector_source::CustomVectorSourceHandle;
pub use geojson::{GeoJSONData, GeoJSONOptions, GeoJsonSourceType, geojson_source};
pub use image::{ImageSourceType, image_source};
pub use tileset::{
    RasterDemSourceType, RasterSourceType, TilesetSourceType, VectorSourceType, raster_dem_source, raster_source,
    vector_source,
};
