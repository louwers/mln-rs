//! Ports `src/mln/map` + `include/mln/map` (Map, Map::Impl, MapOptions,
//! Transform, the observers). The map vocabulary that lower crates need
//! (`CameraOptions`, `MapMode`, `TransformState`, ...) lives in
//! `mln_map_types` and is re-exported here.

pub mod map;
pub mod map_impl;
pub mod map_observer;
pub mod map_options;
// gtests-core: MapProjection (test/map/map.test.cpp).
pub mod map_projection;
pub mod transform;

pub use map::Map;
pub use map_impl::{StillImageCallback, StyleMut};
pub use map_observer::{CameraChangeMode, MapLoadError, MapObserver, NullMapObserver, RenderFrameStatus, RenderMode};
pub use map_options::MapOptions;
pub use map_projection::MapProjection;
pub use mln_map_types::{
    AnimationOptions, BoundOptions, CameraMomentum, CameraOptions, ConstrainMode, FreeCameraOptions, MapDebugOptions,
    MapMode, MomentumDecay, MomentumDecayModel, MomentumEasing, MomentumEnd, MomentumSnap, MomentumVelocity,
    NorthOrientation, ProjectionMode, TileLodMode, TransformState, ViewportMode,
};
