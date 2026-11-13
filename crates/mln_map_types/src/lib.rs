//! The map vocabulary that crates below the map need: camera options and
//! momentum, map modes, bound options, the transform state, and the
//! projections it holds. The `Map` itself is in `mln_map`, which re-exports
//! these.

pub mod bound_options;
pub mod camera;
pub mod camera_momentum;
pub mod frame_projection;
pub mod map_view;
pub mod mercator_projection;
pub mod mode;
pub mod projection_base;
pub mod projection_mode;

pub use bound_options::BoundOptions;
pub use camera::{
    AnimationOptions, CameraDelta, CameraError, CameraOptions, CameraUpdate, CameraUpdateMode, CameraVelocity,
    GesturePhase, TransitionEnd, TransitionEndFn, TransitionOutcome,
};
pub use camera_momentum::{CameraMomentum, MomentumDecay, MomentumDecayModel, MomentumEasing, MomentumSnap};
pub use frame_projection::{ClipDepthRange, FrameProjection};
pub use map_view::{Camera, LogicalExtent, MapView, Viewport};
pub use mode::{ConstrainMode, MapDebugOptions, MapMode, TileLodMode, ViewportMode};
pub use projection_mode::ProjectionMode;
pub mod tile_coordinate;
pub mod tile_projector;
pub mod transform_state;
pub mod util_camera;
pub mod vertical_perspective_projection;

pub use camera::FreeCameraOptions;
pub use mercator_projection::MercatorProjection;
pub use projection_base::{ProjectedTilePoint, ProjectionBase, ProjectionData};
pub use tile_coordinate::TileCoordinate;
pub use tile_projector::TileProjector;
pub use transform_state::{NorthOrientation, TransformState, TransformStateProperties};
pub use vertical_perspective_projection::{CenterAndBearing, VerticalPerspectiveProjection};
