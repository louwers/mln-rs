//! The built-in source types.
//!
//! Each type's module ([`tileset`], [`geojson`], [`image`]) is canonical
//! for its marker type, options and data, and makes its sources.

pub mod geojson;
pub mod image;
pub mod tileset;

pub use geojson::{GeoJSONData, GeoJSONOptions, GeoJsonSourceType, geojson_source};
pub use image::{ImageSourceType, image_source};
pub use tileset::{
    RasterDemSourceType, RasterSourceType, TilesetSourceType, VectorSourceType, raster_dem_source, raster_source,
    vector_source,
};
