//! The map: [`Map`], its options, the transform that moves its camera, and
//! the events it reports. The map vocabulary that lower crates need
//! (`CameraOptions`, `MapMode`, `TransformState`, ...) lives in
//! `mln_map_types` and is re-exported here.

mod feature_state;
mod map;
mod map_event;
mod map_options;
mod map_snapshot;
// gtests-core: MapProjection (test/map/map.test.cpp).
mod map_projection;
mod style_loader;
pub mod transform;

pub use map::{Map, style_camera};
pub use map_event::{CameraChange, CameraChangeMode, CameraChangeSource, MapEvent, RenderFrameStatus};
pub use map_options::MapOptions;
pub use map_projection::MapProjection;
pub use map_snapshot::{CameraBounds, FreeCamera, MapSnapshot, TileSettings};
pub use mln_engine::{
    OverscaledTileID, RenderMode, RendererEvents, RenderingStats, TileOperation, Update, UpdateReader,
};
pub use mln_map_types::{
    AnchoredMomentum, AnimationOptions, AxonometricMode, BoundOptions, Camera, CameraDelta, CameraError, CameraLimits,
    CameraMomentum, CameraOptions, CameraUpdate, CameraUpdateMode, CameraVelocity, ConstrainMode, Elevation,
    FreeCameraOptions, GesturePhase, InvalidMomentum, LogicalExtent, MapMode, MapView, MomentumDecay,
    MomentumDecayModel, MomentumEasing, MomentumGlide, MomentumSnap, NorthOrientation, ProjectionMode, TerrainOptions,
    TerrainSkirtLength, TileLodMode, TransformState, TransitionEnd, TransitionOutcome, View, Viewport, ViewportMode,
};
pub use mln_style::StyleLoadError;
pub use mln_style::error::{Error, ErrorKind};
pub use mln_util::event_queue::Waker;
pub use transform::{CameraCommandId, CommandEnd, FrameClock};

/// The `log` targets of this crate's records. Each names the event that
/// `mln_c` reports the record under (`mln_c::logging`).
mod log_target {
    pub(crate) const GENERAL: &str = "mln_map::general";
    pub(crate) const SETUP: &str = "mln_map::setup";
    pub(crate) const PARSE_STYLE: &str = "mln_map::parse_style";
    pub(crate) const STYLE: &str = "mln_map::style";
    pub(crate) const RENDER: &str = "mln_map::render";
}

#[cfg(test)]
mod tests;
