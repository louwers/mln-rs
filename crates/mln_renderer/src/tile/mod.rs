//! The tiles and their workers. They use the renderer, so they live here;
//! `mln_tile` decodes the tile data.

#[allow(clippy::module_inception)]
pub mod tile;

pub mod custom_tile;
pub mod geojson_tile;
pub mod geojson_tile_data;
pub mod geometry_tile;
pub mod geometry_tile_worker;
pub mod ordered_replies;
pub mod raster_dem_tile; // dem-heatmap
pub mod raster_tile;
pub mod tile_cache;
pub mod tile_loader;
pub mod tile_observer;
pub mod tile_worker_transport;
pub mod vector_tile;
pub mod worker_codec;

pub use tile::{Kind, Tile, TileBase, TileNecessity, TileOperation, TileUpdateParameters};
