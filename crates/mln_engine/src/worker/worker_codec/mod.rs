//! Port-only: tile workers in another wasm instance, through bytes.
//!
//! The browser build runs one wasm instance per page or Web Worker, each
//! with its own memory, so a tile on the map's instance reaches a worker on
//! another instance only through bytes. A tile's job (`Run`, `tile/job.rs`)
//! and its reply cross as frames (envelope.rs): a fixed header with the
//! job's id, then the body, encoded with `mln_wire`. Three things make that
//! work:
//!
//! - Large byte arrays (encoded tiles, decoded images and DEMs, vertex and
//!   index arrays) travel beside a frame's bytes as payloads (`Encoded`,
//!   `mln_wire::payload`), which a host transfers to the other instance
//!   rather than copies.
//! - What many jobs share crosses once per instance, then by key
//!   (`mln_wire::shared`): source styles, layers, their evaluated
//!   properties and their expressions, and sets of images. A
//!   layout refers back to them (its buckets' binders hold the layers'
//!   expressions, its render data the properties). Expressions cross as
//!   their parsed node trees (`mln_style::wire`), and tile data as
//!   what it is made from (`mln_tile::TileData`); an instance keeps the tile
//!   data of its jobs in a cache of its own, so a later job can name it.
//! - Trait objects cross as the types they can be (fields.rs): layers'
//!   style data and evaluated properties, and compound expressions, as
//!   their layer type's or function's name, which the reading side resolves
//!   against its own [`Registry`](crate::Registry); buckets as their bucket
//!   type's.
//!
//! The payloads, the tables of shared values and the registry reach the
//! values that need them through the context that each frame is written or
//! read in (`mln_wire::Context`).
//!
//! Glyphs and images stay on the map side: a job gets where they are in the
//! map's atlas (`Resolved`), never their pixels.
//!
//! `MapCodec` (`map_side.rs`) is the map side of one worker instance and
//! `WorkerCodec` (`worker_side.rs`) that instance's side. `TileWorkerHost`
//! (host.rs) runs an instance's jobs behind its `WorkerCodec`, and
//! `RemoteWorkers` (transport.rs) is the map side of the instances, as
//! `TileWorkers`.

mod envelope;
pub(crate) mod fields;
mod host;
mod map_side;
mod transport;
mod worker_side;

pub use envelope::Encoded;
#[cfg(feature = "bench")]
pub(crate) use envelope::{read_job_id, read_reply_header};
pub use host::TileWorkerHost;
#[cfg(feature = "bench")]
pub(crate) use map_side::MapCodec;
pub(crate) use transport::RemoteEvent;
pub use transport::{RemoteWorkers, ReplySink, ThreadWorkerInstances, WorkerInstances};
#[cfg(feature = "bench")]
pub(crate) use worker_side::WorkerCodec;

#[cfg(test)]
mod tests;
