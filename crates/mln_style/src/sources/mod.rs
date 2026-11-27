//! The style halves of the specification's source types: their marker
//! types, options, style JSON and edits. How their sources load and what
//! they supply the renderer is the engine's half, in `mln_builtins`.
//!
//! Each type's module ([`tileset`], [`geojson`], [`image`]) is canonical
//! for its marker type and options, and makes its sources.

pub mod geojson;
pub mod image;
pub mod tileset;

pub use geojson::{GeoJSONOptions, GeoJsonSourceType, geojson_source};
pub use image::{ImageSourceType, image_source};
pub use tileset::{
    RasterDemSourceType, RasterSourceType, TilesetSourceType, VectorSourceType, raster_dem_source, raster_source,
    vector_source,
};
