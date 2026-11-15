//! The tiles and their workers. They use the renderer, so they live here;
//! `mln_tile` decodes the tile data.

#[expect(
    clippy::module_inception,
    reason = "`tile::tile` holds the `Tile` trait that the other modules' tiles implement"
)]
pub(crate) mod tile;

pub(crate) mod custom_tile;
pub(crate) mod geojson_tile;
pub(crate) mod geometry_tile;
pub(crate) mod job;
pub(crate) mod raster_tile;
pub(crate) mod tile_cache;
pub(crate) mod tile_loader;
pub(crate) mod tile_observer;
pub(crate) mod tile_workers;
pub(crate) mod vector_tile;
pub mod worker_codec;

pub use tile::TileOperation;
