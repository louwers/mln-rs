//! The map: [`MapCore`], its options, the transform that moves its camera, and
//! the events it reports. It runs on one thread, which the runtime's handles
//! drive. The map vocabulary that lower crates need (`CameraOptions`,
//! `MapMode`, `TransformState`, ...) lives in `mln_map_types`.

mod feature_state;
mod map_core;
mod map_event;
mod map_options;
mod map_snapshot;
// gtests-core: MapProjection (test/map/map.test.cpp).
mod map_projection;
mod rendering_stats_view;
mod style_loader;
pub mod transform;

pub use map_core::MapCore;
pub use map_event::{CameraChange, CameraChangeMode, CameraChangeSource, MapEvent, RenderFrameStatus};
pub use map_options::{MapCoreOptions, UnusedStyleImagePolicy};
pub use map_projection::MapProjection;
pub use map_snapshot::{CameraBounds, FreeCamera, MapSnapshot, TileSettings};
pub use transform::{CameraCommandId, CommandEnd, FrameClock};

#[cfg(test)]
mod tests;
