//! The tiles and their workers. They use the renderer, so they live here;
//! `mln_tile` decodes the tile data.

#[allow(clippy::module_inception)]
pub(crate) mod tile;

pub(crate) mod custom_tile;
pub(crate) mod geojson_tile;
pub(crate) mod geojson_tile_data;
pub(crate) mod geometry_tile;
pub(crate) mod geometry_tile_worker;
pub(crate) mod ordered_replies;
pub(crate) mod raster_dem_tile; // dem-heatmap
pub(crate) mod raster_tile;
pub(crate) mod tile_cache;
pub(crate) mod tile_loader;
pub(crate) mod tile_observer;
pub(crate) mod tile_worker_transport;
pub(crate) mod vector_tile;
pub mod worker_codec;

pub use tile::TileOperation;
