//! Settings for the test tools (`tools/render-test`) that no host has a use
//! for. Built only with the `testing` feature, and hidden from the
//! documentation: they aren't API.

use std::sync::atomic::Ordering;

use crate::Runtime;

/// Puts the tile workers of the renderers that `runtime`'s render sessions
/// make from now on in `instances` worker instances, which the renderer
/// reaches only through the worker codec, as the browser build's Web
/// Workers are: every job, bucket and tile crosses as bytes. 0 runs them on
/// the runtime's pool again. A session that is attached keeps its workers.
pub fn set_tile_worker_codec(runtime: &Runtime, instances: usize) {
    runtime.shared.tile_worker_instances.store(instances, Ordering::SeqCst);
}
