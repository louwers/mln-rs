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
pub mod transform;

pub use map::Map;
pub use map_event::{CameraChange, CameraChangeMode, CameraChangeSource, MapEvent, RenderFrameStatus, RenderMode};
pub use map_options::MapOptions;
pub use map_projection::MapProjection;
pub use map_snapshot::MapSnapshot;
pub use mln_map_types::{
    AnimationOptions, BoundOptions, Camera, CameraDelta, CameraError, CameraMomentum, CameraOptions, CameraUpdate,
    CameraUpdateMode, CameraVelocity, ConstrainMode, FreeCameraOptions, GesturePhase, LogicalExtent, MapDebugOptions,
    MapMode, MapView, MomentumDecay, MomentumDecayModel, MomentumEasing, MomentumSnap, NorthOrientation,
    ProjectionMode, TileLodMode, TransformState, TransitionEnd, TransitionOutcome, Viewport, ViewportMode,
};
pub use mln_renderer::{
    FeatureInfo, FrameRenderedFeaturesMap, LayerFeaturesMap, NDCBound, OverscaledTileID, RendererEvents,
    RenderingStats, SourceLayerID, TileOperation, Update, UpdateReader,
};
pub use mln_style::StyleLoadError;
pub use mln_util::event_queue::Waker;
pub use mln_util::{Error, ErrorKind};

#[cfg(test)]
mod tests;
