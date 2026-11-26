//! The built-in source types.
//!
//! Each type's module ([`tileset`], [`geojson`], [`image`], [`custom`]) is
//! canonical for its marker type, options and data, and makes its sources.
//! The custom sources' modules re-export the types their callers name, such
//! as the handles that their hosts answer tile requests through.

pub mod custom;
pub mod custom_geometry_source;
pub mod custom_vector_source;
pub mod geojson;
pub mod image;
pub mod tileset;

pub use custom::{
    CustomGeometrySourceHandle, CustomGeometrySourceType, CustomSourceHandle, CustomVectorSourceHandle,
    CustomVectorSourceType, TileDataFormat,
};
pub use geojson::{GeoJSONData, GeoJSONOptions, GeoJsonSourceType, geojson_source};
pub use image::{ImageSourceType, image_source};
pub use tileset::{
    RasterDemSourceType, RasterSourceType, TilesetSourceType, VectorSourceType, raster_dem_source, raster_source,
    vector_source,
};
