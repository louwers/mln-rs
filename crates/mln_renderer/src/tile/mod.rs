//! The tiles, their store and their workers. They use the renderer, so they
//! live here; `mln_tile` decodes the tile data.

#[expect(clippy::module_inception, reason = "`tile::tile` holds the `Tile` that the store keeps")]
pub(crate) mod tile;

pub(crate) mod contents;
pub(crate) mod dem;
pub(crate) mod inbox;
pub(crate) mod job;
pub(crate) mod loading;
pub(crate) mod pipeline;
pub(crate) mod raster;
pub(crate) mod source_tiles;
pub(crate) mod store;
pub(crate) mod tile_mut;
pub(crate) mod tile_workers;
pub mod worker_codec;

pub use tile::TileOperation;
