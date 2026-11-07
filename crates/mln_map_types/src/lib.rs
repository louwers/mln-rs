//! Ports the `include/mln/map` vocabulary that crates below the map need
//! (cut point, like `mln_style_types`): `camera.hpp` (style's default
//! camera), `mode.hpp`, `projection_mode.hpp`, `bound_options.hpp`, and the
//! projection seam `TransformState` holds (`projection_base.hpp`,
//! `mercator_projection.hpp`). The `Map` itself is in `mln_map`, which
//! re-exports these.

pub mod bound_options;
pub mod camera;
pub mod mercator_projection;
pub mod mode;
pub mod projection_base;
pub mod projection_mode;

pub use bound_options::BoundOptions;
pub use camera::{AnimationOptions, CameraOptions};
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
