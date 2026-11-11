//! Ports `src/mln/map` + `include/mln/map` (Map, MapOptions, Transform)
//! with the events that replace the map's observer. The map vocabulary that lower crates need
//! (`CameraOptions`, `MapMode`, `TransformState`, ...) lives in
//! `mln_map_types` and is re-exported here.

pub mod map;
pub mod map_event;
pub mod map_options;
pub mod map_snapshot;
// gtests-core: MapProjection (test/map/map.test.cpp).
pub mod map_projection;
pub mod rendering_stats_view;
pub mod transform;

pub use map::{Map, StillImageCallback};
pub use map_event::{CameraChange, CameraChangeMode, CameraChangeSource, MapEvent, RenderFrameStatus, RenderMode};
pub use map_options::{MapOptions, UnusedStyleImagePolicy};
pub use map_projection::MapProjection;
pub use map_snapshot::{CameraBounds, FreeCamera, MapSnapshot, TileSettings};
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
