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
//!   `mln_util::serialization::payload`), which a host transfers to the
//!   other instance rather than copies.
//! - What many jobs share crosses once per instance, then by key
//!   (`mln_util::serialization::shared`): source styles, layers, their
//!   evaluated properties and their expressions, and sets of images. A
//!   layout refers back to them (its buckets' binders hold the layers'
//!   expressions, its render data the properties). Expressions cross as
//!   their parsed node trees (`mln_style::serialization`), and tile data as
//!   what it is made from (`mln_tile::TileData`); an instance keeps the tile
//!   data of its jobs in a cache of its own, so a later job can name it.
//! - Trait objects cross as the types they can be (fields.rs): layers'
//!   style data and evaluated properties, and compound expressions, as
//!   their layer type's or function's name, which the reading side resolves
//!   against its own [`Registry`]; buckets as their bucket type's.
//!
//! Glyphs and images stay on the map side: a job gets where they are in the
//! map's atlas (`Resolved`), never their pixels.
//!
//! `MapCodec` (`map_side.rs`) is the map side of one worker instance and
//! `WorkerCodec` (`worker_side.rs`) that instance's side. `TileWorkerHost`
//! (host.rs) runs an instance's jobs behind its `WorkerCodec`, and
//! `RemoteWorkers` (transport.rs) is the map side of the instances, as
//! `TileWorkers`.

use std::cell::Cell;
use std::sync::Arc;

use crate::Registry;
use crate::layer_type::ErasedLayerType;

mod envelope;
pub(crate) mod fields;
mod host;
mod map_side;
mod transport;
mod worker_side;

pub use envelope::Encoded;
pub use host::TileWorkerHost;
pub(crate) use transport::RemoteJobs;
pub use transport::{RemoteWorkers, ReplySink, ThreadWorkerInstances, WorkerInstances};

thread_local! {
    /// The registry of the enclosing `session` on this thread.
    static DECODING: Cell<Option<Arc<Registry>>> = const { Cell::new(None) };
}

/// Runs `f`, in which the values that the codec reads resolve the
/// registrations they name against `registry`. Sessions nest: the innermost
/// wins.
fn session<R>(registry: &Arc<Registry>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Arc<Registry>>);

    impl Drop for Restore {
        fn drop(&mut self) {
            DECODING.set(self.0.take());
        }
    }

    let _restore = Restore(DECODING.replace(Some(Arc::clone(registry))));
    mln_style::decode_with(registry.style(), f)
}

/// The layer type named `name` in the registry of the enclosing `session`.
fn decoding_layer_type(name: &str) -> Result<&'static dyn ErasedLayerType, &'static str> {
    let registry = DECODING.take();
    let layer_type = match &registry {
        None => Err("a layer outside of a decoding session"),
        Some(registry) => registry.layer_type(name).ok_or("unknown layer type"),
    };
    DECODING.set(registry);
    layer_type
}

#[cfg(test)]
mod tests;
