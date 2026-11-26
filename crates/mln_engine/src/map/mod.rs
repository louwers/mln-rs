//! The map: [`Map`], its options, its snapshots, its style's loading and
//! feature state, and the events it reports. Its camera is in `camera`.

#[expect(clippy::module_inception, reason = "`map::map` holds the `Map` that the module is about")]
pub(crate) mod map;

pub(crate) mod events;
pub(crate) mod feature_state;
pub(crate) mod options;
pub(crate) mod snapshot;
pub(crate) mod style_loader;

pub use events::{CameraChange, CameraChangeMode, CameraChangeSource, MapEvent, RenderFrameStatus};
pub use map::{Map, style_camera};
pub use options::{MapMode, MapOptions, TerrainOptions, TerrainSkirtLength};
pub use snapshot::{CameraBounds, FreeCamera, MapSnapshot, TileSettings};

#[cfg(test)]
mod tests;
