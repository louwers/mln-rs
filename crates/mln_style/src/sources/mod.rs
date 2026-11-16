//! The built-in source types and the handles that load their sources.
//!
//! Each type's module ([`tileset`], [`geojson`], [`image`], [`custom`]) is
//! canonical for its marker type, options and data. The `*_source` modules
//! hold the handles, and re-export the types their callers name.

pub mod custom;
pub mod custom_geometry_source;
pub mod custom_vector_source;
pub mod geojson;
pub mod geojson_source;
pub mod image;
pub mod image_source;
pub mod tile_source;
pub mod tileset;

pub use custom::{CustomGeometrySourceType, CustomVectorSourceType, TileDataFormat};
pub use custom_geometry_source::{CustomGeometrySource, CustomGeometrySourceHandle};
pub use custom_vector_source::CustomVectorSource;
pub use geojson::{GeoJSONData, GeoJSONOptions, GeoJsonSourceType};
pub use geojson_source::GeoJSONSource;
pub use image::ImageSourceType;
pub use tile_source::TileSource;
pub use tileset::{RasterDemSourceType, RasterSourceType, TilesetSourceType, VectorSourceType};
