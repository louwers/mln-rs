//! Ports the high files of `src/mln/tile` (PLAN.md, M3: they include the
//! renderer, so they live here; `mln_tile` holds the tile data decoding).
//!
//! TODO(port): raster-dem tiles, custom geometry tiles.

#[allow(clippy::module_inception)]
pub mod tile;

pub mod geojson_tile;
pub mod geojson_tile_data;
pub mod geometry_tile;
pub mod geometry_tile_worker;
pub mod raster_dem_tile; // dem-heatmap
pub mod raster_tile;
pub mod tile_cache;
pub mod tile_loader;
pub mod tile_observer;
pub mod tile_worker_transport;
pub mod vector_tile;
pub mod worker_codec;

pub use tile::{Kind, Tile, TileBase, TileNecessity, TileOperation, TileUpdateParameters};
