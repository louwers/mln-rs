//! Rendering without a window: `HeadlessFrontend` and `MapSnapshotter`,
//! ported from C++'s `platform/default`.

pub mod headless_frontend;
pub mod map_snapshotter;

pub use headless_frontend::{HeadlessFrontend, RenderResult};
pub use map_snapshotter::{MapSnapshotter, MapSnapshotterObserver};
