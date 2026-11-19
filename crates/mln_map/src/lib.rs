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
mod rendering_stats_view;
mod style_loader;
pub mod transform;

pub use map::Map;
pub use map_event::{CameraChange, CameraChangeMode, CameraChangeSource, MapEvent, RenderFrameStatus};
pub use map_options::MapOptions;
pub use map_projection::MapProjection;
pub use map_snapshot::{CameraBounds, FreeCamera, MapSnapshot, TileSettings};
pub use mln_map_types::{
    AnchoredMomentum, AnimationOptions, BoundOptions, Camera, CameraDelta, CameraError, CameraLimits, CameraMomentum,
    CameraOptions, CameraUpdate, CameraUpdateMode, CameraVelocity, ConstrainMode, FreeCameraOptions, GesturePhase,
    InvalidMomentum, LogicalExtent, MapDebugOptions, MapMode, MapView, MomentumDecay, MomentumDecayModel,
    MomentumEasing, MomentumGlide, MomentumSnap, NorthOrientation, ProjectionMode, TileLodMode, TransformState,
    TransitionEnd, TransitionOutcome, View, Viewport, ViewportMode,
};
pub use mln_renderer::{
    OverscaledTileID, RenderMode, RendererEvents, RenderingStats, TileOperation, Update, UpdateReader,
};
pub use mln_style::StyleLoadError;
pub use mln_util::event_queue::Waker;
pub use mln_util::{Error, ErrorKind};
pub use transform::{CameraCommandId, CommandEnd, FrameClock};

#[cfg(test)]
mod tests;
