//! The map vocabulary that crates below the map need: camera options and
//! momentum, map modes, bound options, the transform state, the view it
//! shows and draws with, and the tiles that cover a view, bounds or a
//! geometry ([`tile_cover`]). The map itself is `mln_runtime`'s.

mod bound_options;
mod camera;
mod camera_limits;
mod camera_momentum;
mod elevation;
mod frame_projection;
mod ground;
mod map_view;
mod mode;
mod projection;
mod projection_mode;

pub use bound_options::BoundOptions;
pub use camera::{
    AnimationOptions, CameraDelta, CameraError, CameraOptions, CameraUpdate, CameraUpdateMode, CameraVelocity,
    GesturePhase, TransitionEnd, TransitionOutcome,
};
pub use camera_limits::{CameraLimits, MAX_FIELD_OF_VIEW, MIN_FIELD_OF_VIEW};
pub use camera_momentum::{
    AnchoredMomentum, CameraMomentum, InvalidMomentum, MomentumDecay, MomentumDecayModel, MomentumEasing,
    MomentumGlide, MomentumSnap,
};
pub use elevation::Elevation;
pub use frame_projection::{ClipDepthRange, FrameProjection};
pub use ground::GroundHit;
pub use map_view::{Camera, LogicalExtent, MapView, Viewport};
pub use mode::{ConstrainMode, MapDebugOptions, MapMode, TileLodMode, ViewportMode};
pub use projection_mode::ProjectionMode;
mod camera_pose;
pub mod tile_coordinate;
pub mod tile_cover;
mod tile_cover_impl;
mod tile_lod;
mod tile_projector;
mod transform_state;
mod vertical_perspective_projection;
mod view;

pub use camera::FreeCameraOptions;
pub use projection::{ProjectedTilePoint, Projection, ProjectionData};
pub use tile_coordinate::TileCoordinate;
pub use tile_projector::TileProjector;
pub use transform_state::TransformState;
pub use vertical_perspective_projection::{CenterAndBearing, VerticalPerspectiveProjection};
pub use view::{NorthOrientation, View, scale_zoom, zoom_scale};

#[cfg(test)]
mod tile_lod_test;
