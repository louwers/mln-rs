//! Port-only: the messages between a tile and its worker as bytes, for a
//! worker in another wasm instance.
//!
//! The browser build runs one wasm instance per page or Web Worker, each
//! with its own memory, so a tile on the map's instance reaches a worker on
//! another instance only through bytes. The messages of geometry tiles
//! (`WorkerMessage` and `TileMessage`, `geometry_tile_worker.rs`), raster tiles
//! (`RasterWorkerMessage` and `RasterTileMessage`, `raster_tile.rs`) and
//! raster-dem tiles (`RasterDEMWorkerMessage` and `RasterDEMTileMessage`,
//! `raster_dem_tile.rs`) derive `mln_wire::Wire`, as do the types they carry,
//! and cross encoded inside a small envelope (envelope.rs). Three things
//! make that work:
//!
//! - Large byte arrays (encoded tiles, decoded images and DEMs, vertex and
//!   index arrays) travel beside a reply's bytes as payloads (`Encoded`,
//!   `mln_util::serialization::payload`), which a host transfers to the map's
//!   instance rather than copies.
//! - What many messages share crosses once per instance, then by key
//!   (`mln_util::serialization::shared)`: layers, their evaluated properties
//!   and their expressions, and the bytes or features of tile data from the
//!   map side, which a layout refers back to (its buckets' binders hold the
//!   layers' expressions, its render data the properties, its feature index
//!   the data). Expressions cross as their parsed node trees
//!   (`mln_style::serialization`), and tile data as what it is made from
//!   (`mln_tile::TileData`).
//! - Trait objects cross as the types they can be (fields.rs): layers and
//!   their properties as their layer type's, buckets as their bucket type's.
//!
//! Glyphs and images stay on the map side: a worker gets where they are in
//! the map's atlas (`WorkerMessage::Resolved`), never their pixels.
//!
//! `MapCodec` (`map_side.rs`) is the map side of one worker instance and
//! `WorkerCodec` (`worker_side.rs`) that instance's side. `TileWorkerHost`
//! (host.rs) runs an instance's workers behind its `WorkerCodec`, and
//! `SerializingTransport` (transport.rs) is the `TileWorkerTransport` that
//! puts the tiles' workers in instances, through their `MapCodec`s.

mod envelope;
pub(crate) mod fields;
mod host;
mod map_side;
mod transport;
mod worker_side;

pub use envelope::Encoded;
pub use host::TileWorkerHost;
pub use transport::{SerializingTransport, ThreadWorkerInstances, WorkerInstances};

#[cfg(test)]
mod host_tests;
#[cfg(test)]
mod raster_tests;
#[cfg(test)]
mod tests;
