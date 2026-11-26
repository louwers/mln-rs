//! The camera: its options, momentum and limits, bound options, the
//! transform that moves it, the transform state, and the view it shows and
//! draws with.

mod bound_options;
#[expect(clippy::module_inception, reason = "`camera::camera` holds the camera's options and updates")]
mod camera;
mod camera_limits;
mod camera_momentum;
mod elevation;
mod frame_projection;
mod ground;
// gtests-core: MapProjection (test/map/map.test.cpp).
pub(crate) mod map_projection;
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
pub use map_projection::MapProjection;
pub use map_view::{Camera, LogicalExtent, MapView, Viewport};
pub use mode::{ConstrainMode, ViewportMode};
pub use projection_mode::{AxonometricMode, ProjectionMode};
mod camera_pose;
pub mod tile_coordinate;
mod tile_projector;
pub mod transform;
mod transform_state;
mod vertical_perspective_projection;
mod view;

pub use camera::FreeCameraOptions;
pub use projection::{ProjectedTilePoint, Projection, ProjectionData};
pub use tile_coordinate::TileCoordinate;
pub use tile_projector::TileProjector;
pub use transform::{CameraCommandId, CommandEnd, FrameClock};
pub use transform_state::TransformState;
pub use vertical_perspective_projection::{CenterAndBearing, VerticalPerspectiveProjection};
pub use view::{NorthOrientation, View, scale_zoom, zoom_scale};
