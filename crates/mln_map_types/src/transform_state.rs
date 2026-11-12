// Ports src/mln/map/transform_state.hpp and src/mln/map/transform_state.cpp.
//
// The matrices are computed when a getter first needs them and cached in a
// `OnceLock`, which each change that moves them clears, so a state shared
// between threads reads them without locks. The camera follows from the
// other fields, so it is computed where it's needed rather than cached.
//
// `updateMatricesIfNeeded` throws `std::runtime_error` when a matrix can't be
// inverted; nothing catches it on the paths that run, so the port panics
// with the same message.

use std::f64::consts::PI;
use std::sync::{Arc, OnceLock};

use geometry::Point;
use mln_style_types::ProjectionDefinition;
use mln_tile_id::UnwrappedTileID;
use mln_util::constants;
use mln_util::constants::{DEFAULT_FOV, DEFAULT_MAX_ZOOM, DEFAULT_PITCH_MAX, M2PI, PITCH_MAX, TILE_SIZE_D};
use mln_util::error::Error;
pub use mln_util::geo::NorthOrientation;
use mln_util::geo::{EdgeInsets, LatLng, LatLngBounds, ScreenCoordinate, WrapMode};
use mln_util::interpolate::interpolate;
use mln_util::logging::{Event, Log};
use mln_util::mat4::{Mat4, matrix};
use mln_util::projection::Projection;
use mln_util::quaternion::Quaternion;
use mln_util::size::Size;
use mln_util::vectors::{Vec3, Vec4};

use crate::camera::{CameraOptions, FreeCameraOptions};
use crate::map_view::Camera as MapCamera;
use crate::mercator_projection::MercatorProjection;
use crate::mode::{ConstrainMode, ViewportMode};
use crate::projection_base::{ProjectionBase, ProjectionData};
use crate::tile_coordinate::TileCoordinate;
use crate::util_camera::Camera;
use crate::vertical_perspective_projection::VerticalPerspectiveProjection;

/// The maximum angle to use for the Mercator horizon. This must be less than
/// 90 to prevent errors in `MercatorTransform::_calcMatrices()`. It shouldn't
/// be too close to 90, or the distance to the horizon will become very large,
/// unnecessarily increasing the number of tiles needed to render the map.
fn max_mercator_horizon_angle() -> f64 {
    mln_math::deg2rad(89.25)
}

fn lat_lng(lat: f64, lon: f64, mode: WrapMode) -> LatLng {
    // C++ constructs LatLng directly, which throws std::domain_error on
    // invalid input; nothing catches it there.
    LatLng::with_mode(lat, lon, mode).unwrap_or_else(|e| panic!("{e}"))
}

fn lat_lng_from_mercator(mercator_coordinate: Point<f64>, wrap_mode: WrapMode) -> LatLng {
    lat_lng(
        mln_math::rad2deg(2.0 * (PI - mercator_coordinate.y * 2.0 * PI).exp().atan() - (PI / 2.0)),
        mercator_coordinate.x * 360.0 - 180.0,
        wrap_mode,
    )
}

const K_EPSILON: f64 = 1e-9;

/// To avoid flickering issue due to "zoom = 13.9999999..".
fn round_for_accuracy(x: f64) -> f64 {
    let round_x = x.round();
    let diff = (round_x - x).abs();
    if diff < K_EPSILON && diff > 0.0 { round_x } else { x }
}

/// `std::min` (returns `a` unless `b < a`).
fn std_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// `std::max` (returns `a` unless `a < b`).
fn std_max(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

/// `std::modf` fractional part (same sign as `x`).
fn modf_fraction(x: f64) -> f64 {
    if x.is_infinite() {
        return 0.0f64.copysign(x);
    }
    (x - x.trunc()).copysign(x)
}

/// `TransformStateProperties`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TransformStateProperties {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub z: Option<f64>,
    pub fov: Option<f64>,
    pub scale: Option<f64>,
    pub bearing: Option<f64>,
    pub pitch: Option<f64>,
    pub roll: Option<f64>,
    pub x_skew: Option<f64>,
    pub y_skew: Option<f64>,
    pub axonometric: Option<bool>,
    pub panning: Option<bool>,
    pub scaling: Option<bool>,
    pub rotating: Option<bool>,
    pub edge_insets: Option<EdgeInsets>,
    pub size: Option<Size>,
    pub constrain: Option<ConstrainMode>,
    pub north_orientation: Option<NorthOrientation>,
    pub view_port_mode: Option<ViewportMode>,
    pub frustum_offset: Option<EdgeInsets>,
}

impl TransformStateProperties {
    #[must_use]
    pub fn with_x(mut self, val: Option<f64>) -> Self {
        self.x = val;
        self
    }
    #[must_use]
    pub fn with_y(mut self, val: Option<f64>) -> Self {
        self.y = val;
        self
    }
    #[must_use]
    pub fn with_z(mut self, val: Option<f64>) -> Self {
        self.z = val;
        self
    }
    #[must_use]
    pub fn with_scale(mut self, val: Option<f64>) -> Self {
        self.scale = val;
        self
    }
    #[must_use]
    pub fn with_fov(mut self, val: Option<f64>) -> Self {
        self.fov = val;
        self
    }
    #[must_use]
    pub fn with_bearing(mut self, val: Option<f64>) -> Self {
        self.bearing = val;
        self
    }
    #[must_use]
    pub fn with_pitch(mut self, val: Option<f64>) -> Self {
        self.pitch = val;
        self
    }
    #[must_use]
    pub fn with_roll(mut self, val: Option<f64>) -> Self {
        self.roll = val;
        self
    }
    #[must_use]
    pub fn with_x_skew(mut self, val: Option<f64>) -> Self {
        self.x_skew = val;
        self
    }
    #[must_use]
    pub fn with_y_skew(mut self, val: Option<f64>) -> Self {
        self.y_skew = val;
        self
    }
    #[must_use]
    pub fn with_axonometric(mut self, val: Option<bool>) -> Self {
        self.axonometric = val;
        self
    }
    #[must_use]
    pub fn with_panning_in_progress(mut self, val: Option<bool>) -> Self {
        self.panning = val;
        self
    }
    #[must_use]
    pub fn with_scaling_in_progress(mut self, val: Option<bool>) -> Self {
        self.scaling = val;
        self
    }
    #[must_use]
    pub fn with_rotating_in_progress(mut self, val: Option<bool>) -> Self {
        self.rotating = val;
        self
    }
    #[must_use]
    pub fn with_edge_insets(mut self, val: Option<EdgeInsets>) -> Self {
        self.edge_insets = val;
        self
    }
    #[must_use]
    pub fn with_size(mut self, val: Option<Size>) -> Self {
        self.size = val;
        self
    }
    #[must_use]
    pub fn with_constrain_mode(mut self, val: Option<ConstrainMode>) -> Self {
        self.constrain = val;
        self
    }
    #[must_use]
    pub fn with_north_orientation(mut self, val: Option<NorthOrientation>) -> Self {
        self.north_orientation = val;
        self
    }
    #[must_use]
    pub fn with_viewport_mode(mut self, val: Option<ViewportMode>) -> Self {
        self.view_port_mode = val;
        self
    }
    #[must_use]
    pub fn with_frustum_offset(mut self, val: Option<EdgeInsets>) -> Self {
        self.frustum_offset = val;
        self
    }
}

#[derive(Clone, Debug)]
pub struct TransformState {
    bounds: LatLngBounds,

    // Limit the amount of zooming possible on the map.
    min_scale: f64,
    max_scale: f64,

    // Limit the amount of pitch
    min_pitch: f64,
    max_pitch: f64,
    min_fov: f64,
    max_fov: f64,

    orientation: NorthOrientation,

    // logical dimensions
    size: Size,
    frustum_offset: EdgeInsets,

    projection: Arc<dyn ProjectionBase>,
    /// The style's projection that the state draws, as it was last set.
    projection_definition: ProjectionDefinition,
    projection_transition: f64,

    constrain_mode: ConstrainMode,
    viewport_mode: ViewportMode,

    // animation state
    rotating: bool,
    scaling: bool,
    panning: bool,
    gesture_in_progress: bool,

    // map position
    x: f64,
    y: f64,
    z: f64,
    bearing: f64,
    scale: f64,
    fov: f64,
    pitch: f64,
    roll: f64,
    x_skew: f64,
    y_skew: f64,
    axonometric: bool,

    edge_insets: EdgeInsets,

    // cache values for spherical mercator math
    bc: f64,
    cc: f64,

    matrices: OnceLock<Matrices>,
}

/// The matrices of a state, zero while its size is empty. The globe's are
/// zero unless it renders the globe.
#[derive(Clone, Copy, Debug, Default)]
struct Matrices {
    projection: Mat4,
    inv_projection: Mat4,
    coord: Mat4,
    inverted: Mat4,
    globe_radius: f64,
    globe_view_projection: Mat4,
    inv_globe_view_projection: Mat4,
    globe_clipping_plane: Vec4,
    globe_camera_position: Vec3,
}

impl Default for TransformState {
    fn default() -> Self {
        TransformState::new(ConstrainMode::HeightOnly, ViewportMode::Default)
    }
}

impl TransformState {
    #[must_use]
    pub fn new(constrain_mode: ConstrainMode, viewport_mode: ViewportMode) -> Self {
        let scale = 1.0;
        TransformState {
            bounds: LatLngBounds::default(),
            min_scale: 2.0f64.powf(0.0),
            max_scale: 2.0f64.powf(f64::from(DEFAULT_MAX_ZOOM)),
            min_pitch: constants::PITCH_MIN,
            max_pitch: DEFAULT_PITCH_MAX,
            min_fov: mln_math::deg2rad(0.1),
            max_fov: mln_math::deg2rad(150.0),
            orientation: NorthOrientation::Upwards,
            size: Size::default(),
            frustum_offset: EdgeInsets::default(),
            projection: Arc::new(MercatorProjection),
            projection_definition: ProjectionDefinition::default(),
            projection_transition: 0.0,
            constrain_mode,
            viewport_mode,
            rotating: false,
            scaling: false,
            panning: false,
            gesture_in_progress: false,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            bearing: 0.0,
            scale,
            fov: DEFAULT_FOV,
            pitch: 0.0,
            roll: 0.0,
            x_skew: 0.0,
            y_skew: 1.0,
            axonometric: false,
            edge_insets: EdgeInsets::default(),
            bc: Projection::world_size(scale) / constants::DEGREES_MAX,
            cc: Projection::world_size(scale) / M2PI,
            matrices: OnceLock::new(),
        }
    }

    pub fn set_properties(&mut self, properties: &TransformStateProperties) {
        if let Some(x) = properties.x {
            self.set_x(x);
        }
        if let Some(y) = properties.y {
            self.set_y(y);
        }
        if let Some(z) = properties.z {
            self.set_z(z);
        }
        if let Some(scale) = properties.scale {
            self.set_scale(scale);
        }
        if let Some(bearing) = properties.bearing {
            self.set_bearing(bearing);
        }
        if let Some(fov) = properties.fov {
            self.set_field_of_view(fov);
        }
        if let Some(pitch) = properties.pitch {
            self.set_pitch(pitch);
        }
        if let Some(roll) = properties.roll {
            self.set_roll(roll);
        }
        if let Some(x_skew) = properties.x_skew {
            self.set_x_skew(x_skew);
        }
        if let Some(y_skew) = properties.y_skew {
            self.set_y_skew(y_skew);
        }
        if let Some(axonometric) = properties.axonometric {
            self.set_axonometric(axonometric);
        }
        if let Some(panning) = properties.panning {
            self.set_panning_in_progress(panning);
        }
        if let Some(scaling) = properties.scaling {
            self.set_scaling_in_progress(scaling);
        }
        if let Some(rotating) = properties.rotating {
            self.set_rotating_in_progress(rotating);
        }
        if let Some(edge_insets) = properties.edge_insets {
            self.set_edge_insets(&edge_insets);
        }
        if let Some(size) = properties.size {
            self.set_size(size);
        }
        if let Some(constrain) = properties.constrain {
            self.set_constrain_mode(constrain);
        }
        if let Some(north_orientation) = properties.north_orientation {
            self.set_north_orientation(north_orientation);
        }
        if let Some(view_port_mode) = properties.view_port_mode {
            self.set_viewport_mode(view_port_mode);
        }
        if let Some(frustum_offset) = properties.frustum_offset {
            self.set_frustum_offset(&frustum_offset);
        }
    }

    // MARK: - Projection

    pub fn set_projection_definition(&mut self, definition: &ProjectionDefinition) {
        self.projection_definition = *definition;
        let transition = definition.transition_state();
        if transition == self.projection_transition {
            return;
        }
        let leaves_globe = transition == 0.0 && self.projection_transition > 0.0;
        if (transition > 0.0) != (self.projection_transition > 0.0) {
            if transition > 0.0 {
                self.projection = Arc::new(VerticalPerspectiveProjection);
            } else {
                self.projection = Arc::new(MercatorProjection);
            }
        }
        self.projection_transition = transition;
        self.invalidate_matrices();
        // As GL JS's `migrateProjection` does: the globe allows lower zooms,
        // latitudes past Mercator's and a center on another world copy, so
        // Mercator's limits and its wrapped center apply again.
        if leaves_globe && !self.size.is_empty() {
            let wrap_mode =
                if self.bounds == LatLngBounds::default() { WrapMode::Wrapped } else { WrapMode::Unwrapped };
            let center = self.get_lat_lng(wrap_mode);
            let zoom = self.get_zoom();
            self.set_lat_lng_zoom(&center, zoom);
        }
    }

    /// The definition that [`set_projection_definition`](Self::set_projection_definition)
    /// last set.
    pub fn get_projection_definition(&self) -> ProjectionDefinition {
        self.projection_definition
    }

    /// 0 is Mercator, 1 is the globe, in between is the transition.
    pub fn get_projection_transition(&self) -> f64 {
        self.projection_transition
    }

    pub fn is_globe_rendering(&self) -> bool {
        self.projection_transition > 0.0
    }

    // MARK: - Matrix

    pub fn matrix_for(&self, matrix: &mut Mat4, tile_id: &UnwrappedTileID) {
        self.projection.tile_matrix(matrix, tile_id, self.scale);
    }

    pub fn get_projection(&self) -> &dyn ProjectionBase {
        self.projection.as_ref()
    }

    /// `getProjectionData(const UnwrappedTileID&, const mat4& projMatrix)`.
    pub fn get_projection_data_with(&self, tile_id: &UnwrappedTileID, proj_matrix: &Mat4) -> ProjectionData {
        let mut mercator_matrix: Mat4 = [0.0; 16];
        self.projection.tile_matrix(&mut mercator_matrix, tile_id, self.scale);
        let a = mercator_matrix;
        matrix::multiply(&mut mercator_matrix, proj_matrix, &a);
        self.projection.get_projection_data(self, tile_id, &mercator_matrix)
    }

    /// `getProjectionData(const UnwrappedTileID&)`.
    pub fn get_projection_data(&self, tile_id: &UnwrappedTileID) -> ProjectionData {
        self.get_projection_data_with(tile_id, &self.get_projection_matrix())
    }

    pub fn get_projection_data_for_matrix(&self, tile_id: &UnwrappedTileID, mercator_matrix: &Mat4) -> ProjectionData {
        self.projection.get_projection_data(self, tile_id, mercator_matrix)
    }

    /// `getProjMatrix(mat4&, uint16_t nearZ = 1, bool aligned = false)`.
    pub fn get_proj_matrix(&self, proj_matrix: &mut Mat4, near_z: u16, aligned: bool) {
        if self.size.is_empty() {
            return;
        }

        let camera_to_center_distance = f64::from(self.get_camera_to_center_distance());
        let offset = self.get_center_offset();

        let limited_pitch = mln_math::clamp_f64(self.get_pitch(), 0.0, max_mercator_horizon_angle());
        let camera_to_sea_level_distance = camera_to_center_distance + self.z.abs() / limited_pitch.cos();

        // Find the Z distance from the viewport center point
        // [width/2 + offset.x, height/2 + offset.y] to the top edge; to point
        // [width/2 + offset.x, 0] in Z units.
        // 1 Z unit is equivalent to 1 horizontal px at the center of the map
        // (the distance between[width/2, height/2] and [width/2 + 1, height/2])
        // See https://github.com/mapbox/mapbox-gl-native/pull/15195 for details.
        // See TransformState::fov description: fov = 2 * arctan((height / 2) / (height * 1.5)).
        let width = f64::from(self.size.width);
        let height = f64::from(self.size.height);
        let tan_fov_above_center = (0.5 + (offset.y - self.frustum_offset.top) / height)
            * 2.0
            * (self.fov / 2.0).tan()
            * (self.roll.cos().abs() + self.roll.sin().abs() * width / height);
        let tan_multiple = mln_math::clamp_f64(tan_fov_above_center * limited_pitch.tan(), 0.0, 0.99);
        debug_assert!(tan_multiple < 1.0);
        // Calculate z distance of the farthest fragment that should be rendered.
        let furthest_distance = camera_to_sea_level_distance / (1.0 - tan_multiple);
        // Add a bit extra to avoid precision problems when a fragment's
        // distance is exactly `furthestDistance`
        let far_z = furthest_distance * 1.01;

        let camera = self.camera_state();
        let world_to_camera = camera.get_world_to_camera(self.scale, self.viewport_mode == ViewportMode::FlippedY);
        // getFieldOfView() returns float.
        let mut camera_to_clip = camera.get_camera_to_clip_perspective(
            f64::from(self.get_field_of_view()),
            width / height,
            f64::from(near_z),
            far_z,
        );

        // Move the center of perspective to center of specified edgeInsets.
        // Values are in range [-1, 1] where the upper and lower range values
        // position viewport center to the screen edges. This is overridden
        // if using axonometric perspective (not in public API yet, Issue
        // #11882).
        // TODO(astojilj): Issue #11882 should take edge insets into account, too.
        if !self.axonometric {
            camera_to_clip[8] = -offset.x * 2.0 / width;
            camera_to_clip[9] = offset.y * 2.0 / height;
        }

        // Apply north orientation angle
        if self.get_north_orientation() != NorthOrientation::Upwards {
            let a = camera_to_clip;
            matrix::rotate_z(&mut camera_to_clip, &a, -self.get_north_orientation_angle());
        }

        matrix::multiply(proj_matrix, &camera_to_clip, &world_to_camera);

        if self.axonometric {
            // mat[11] controls perspective
            proj_matrix[11] = 0.0;

            // mat[8], mat[9] control x-skew, y-skew
            let pixels_per_meter = 1.0
                / Projection::get_meters_per_pixel_at_latitude(
                    self.get_lat_lng(WrapMode::Unwrapped).latitude(),
                    self.get_zoom(),
                );
            proj_matrix[8] = self.x_skew * pixels_per_meter;
            proj_matrix[9] = self.y_skew * pixels_per_meter;
        }

        // Make a second projection matrix that is aligned to a pixel grid for
        // rendering raster tiles. We're rounding the (floating point) x/y
        // values to achieve to avoid rendering raster images to fractional
        // coordinates. Additionally, we adjust by half a pixel in either
        // direction in case that viewport dimension is an odd integer to
        // preserve rendering to the pixel grid. We're rotating this shift
        // based on the angle of the transformation so that 0°, 90°, 180°, and
        // 270° rasters are crisp, and adjust the shift so that it is always
        // <= 0.5 pixels.

        if aligned {
            let world_size = Projection::world_size(self.scale);
            let dx = self.x - 0.5 * world_size;
            let dy = self.y - 0.5 * world_size;

            let x_shift = f64::from(self.size.width % 2) / 2.0;
            let y_shift = f64::from(self.size.height % 2) / 2.0;
            let bearing_cos = self.bearing.cos();
            let bearing_sin = self.bearing.sin();
            let dxa = -modf_fraction(dx) + bearing_cos * x_shift + bearing_sin * y_shift;
            let dya = -modf_fraction(dy) + bearing_cos * y_shift + bearing_sin * x_shift;
            let a = *proj_matrix;
            matrix::translate(
                proj_matrix,
                &a,
                if dxa > 0.5 { dxa - 1.0 } else { dxa },
                if dya > 0.5 { dya - 1.0 } else { dya },
                0.0,
            );
        }
    }

    /// The camera that the state's position and orientation describe, or the
    /// default camera while the state isn't valid.
    fn camera_state(&self) -> Camera {
        let mut camera = Camera::new();
        if !self.valid() {
            return camera;
        }

        let world_size = Projection::world_size(self.scale);
        let camera_to_center_distance = f64::from(self.get_camera_to_center_distance());

        // x & y tracks the center of the map in pixels. However as rendering
        // is done in pixel coordinates the rendering origo is actually in the
        // middle of the map (0.5 * worldSize). x&y positions have to be
        // negated because it defines position of the map, not the camera.
        // Moving map 10 units left has the same effect as moving camera 10
        // units to the right.
        let dx = 0.5 * world_size - self.x;
        let dy = 0.5 * world_size - self.y;

        // Set camera orientation and move it to a proper distance from the map
        camera.set_orientation(self.get_roll(), self.get_pitch(), self.get_bearing());

        let forward = camera.forward();
        let orbit_position: Vec3 = [
            -forward[0] * camera_to_center_distance,
            -forward[1] * camera_to_center_distance,
            -forward[2] * camera_to_center_distance,
        ];
        let mut camera_position: Vec3 = [dx + orbit_position[0], dy + orbit_position[1], self.z + orbit_position[2]];

        camera_position[0] /= world_size;
        camera_position[1] /= world_size;
        camera_position[2] /= world_size;

        camera.set_position(&camera_position);
        camera
    }

    fn update_state_from_camera(&mut self, camera: &Camera) {
        let position = camera.get_position();
        let forward = camera.forward();

        let dx = forward[0];
        let dy = forward[1];
        let dz = forward[2];

        // Compute bearing and pitch
        let (new_roll, mut new_pitch, new_bearing) = camera.get_orientation_angles();
        new_pitch = mln_math::clamp_f64(new_pitch, self.min_pitch, self.max_pitch);

        // Compute zoom level from the camera altitude
        let center_distance = f64::from(self.get_camera_to_center_distance());
        let zoom;
        let travel;
        if dz < -1.0e-9 && position[2] > 1.0e-9 && new_pitch <= max_mercator_horizon_angle() {
            zoom = (center_distance / (position[2] / new_pitch.cos() * TILE_SIZE_D)).log2();
            travel = -position[2] / dz;
        } else {
            zoom = 14.0;
            let new_scale = mln_math::clamp_f64(2.0f64.powf(zoom), self.min_scale, self.max_scale);
            travel = center_distance / new_scale / TILE_SIZE_D;
        }

        // Compute center point of the map
        let mercator_point = Point::new(position[0] + dx * travel, position[1] + dy * travel);
        let zoom = std_min(zoom, self.scale_zoom(self.max_scale));
        self.set_lat_lng_zoom(&lat_lng_from_mercator(mercator_point, WrapMode::Unwrapped), zoom);

        let mercator_z = position[2] + dz * travel;
        let alt_m = mercator_z
            * Projection::get_meters_per_pixel_at_latitude(self.get_lat_lng(WrapMode::Unwrapped).latitude(), 0.0)
            * TILE_SIZE_D;
        self.set_center_altitude(alt_m);
        self.set_bearing(new_bearing);
        self.set_pitch(new_pitch);
        self.set_roll(new_roll);
    }

    pub fn get_free_camera_options(&self) -> FreeCameraOptions {
        let camera = self.camera_state();
        FreeCameraOptions { position: Some(camera.get_position()), orientation: Some(camera.get_orientation().m()) }
    }

    fn set_camera_position(&self, camera: &mut Camera, position: &Vec3) -> bool {
        if position[0].is_nan() || position[1].is_nan() || position[2].is_nan() {
            return false;
        }

        let max_world_size = Projection::world_size(2.0f64.powf(self.get_max_zoom()));
        let min_world_size = Projection::world_size(2.0f64.powf(self.get_min_zoom()));
        let dist_to_center = f64::from(self.get_camera_to_center_distance());

        let updated_pos: Vec3 = [
            position[0],
            position[1],
            mln_math::clamp_f64(position[2], dist_to_center / max_world_size, dist_to_center / min_world_size),
        ];

        camera.set_position(&updated_pos);
        true
    }

    fn set_camera_orientation(camera: &mut Camera, orientation: &Quaternion) -> bool {
        let c = orientation.m();
        if c[0].is_nan() || c[1].is_nan() || c[2].is_nan() || c[3].is_nan() {
            return false;
        }

        // Zero-length quaternions are not valid
        if orientation.length() == 0.0 {
            return false;
        }

        let unit_quat = orientation.normalized();
        let forward = unit_quat.transform(&[0.0, 0.0, -1.0]);
        let up = unit_quat.transform(&[0.0, -1.0, 0.0]);

        let Some(updated_orientation) = Camera::orientation_from_frame(&forward, &up) else {
            return false;
        };

        camera.set_orientation_quaternion(&updated_orientation);
        true
    }

    pub fn set_free_camera_options(&mut self, options: &FreeCameraOptions) {
        if !self.valid() {
            return;
        }

        if options.position.is_none() && options.orientation.is_none() {
            return;
        }

        let mut camera = self.camera_state();
        let mut changed = false;
        if let Some(orientation) = options.orientation
            && orientation != camera.get_orientation().m()
        {
            changed |= Self::set_camera_orientation(&mut camera, &Quaternion::from_vec4(&orientation));
        }

        if let Some(position) = options.position
            && position != camera.get_position()
        {
            changed |= self.set_camera_position(&mut camera, &position);
        }

        if changed {
            self.update_state_from_camera(&camera);
            self.invalidate_matrices();
        }
    }

    fn matrices(&self) -> &Matrices {
        self.matrices.get_or_init(|| {
            if self.size.is_empty() {
                return Matrices::default();
            }
            let mut projection = [0.0; 16];
            self.get_proj_matrix(&mut projection, 1, false);
            let coord = self.coordinate_point_matrix(&projection);

            let mut inv_projection = [0.0; 16];
            if matrix::invert(&mut inv_projection, &projection) {
                panic!("failed to invert projectionMatrix");
            }
            let mut inverted = [0.0; 16];
            if matrix::invert(&mut inverted, &coord) {
                panic!("failed to invert coordinatePointMatrix");
            }
            let mut matrices = Matrices { projection, inv_projection, coord, inverted, ..Matrices::default() };
            // The globe's projections read only the state's fields, never
            // these matrices, so they can run while the lock initializes.
            if self.is_globe_rendering() {
                let globe_radius = VerticalPerspectiveProjection::globe_radius_pixels(
                    Projection::world_size(self.scale),
                    self.get_lat_lng(WrapMode::Unwrapped).latitude(),
                );
                let globe_view_projection =
                    VerticalPerspectiveProjection::globe_view_projection_matrix(self, globe_radius);
                let mut inv_globe_view_projection = [0.0; 16];
                if matrix::invert(&mut inv_globe_view_projection, &globe_view_projection) {
                    matrix::identity(&mut inv_globe_view_projection);
                }
                matrices.globe_radius = globe_radius;
                matrices.globe_view_projection = globe_view_projection;
                matrices.inv_globe_view_projection = inv_globe_view_projection;
                matrices.globe_clipping_plane = VerticalPerspectiveProjection::clipping_plane(self, globe_radius);
                matrices.globe_camera_position = VerticalPerspectiveProjection::camera_position(self, globe_radius);
            }
            matrices
        })
    }

    /// The globe's radius in pixels for the current camera, valid while
    /// `is_globe_rendering()`; cached with the matrices, as are the globe
    /// getters below.
    pub fn get_globe_radius_pixels(&self) -> f64 {
        self.matrices().globe_radius
    }

    pub fn get_globe_view_projection_matrix(&self) -> Mat4 {
        self.matrices().globe_view_projection
    }

    pub fn get_inverse_globe_view_projection_matrix(&self) -> Mat4 {
        self.matrices().inv_globe_view_projection
    }

    pub fn get_globe_clipping_plane(&self) -> Vec4 {
        self.matrices().globe_clipping_plane
    }

    pub fn get_globe_camera_position(&self) -> Vec3 {
        self.matrices().globe_camera_position
    }

    /// Clears the matrices after a change that moves them.
    fn invalidate_matrices(&mut self) {
        self.matrices.take();
    }

    pub fn get_projection_matrix(&self) -> Mat4 {
        self.matrices().projection
    }

    pub fn get_inv_projection_matrix(&self) -> Mat4 {
        self.matrices().inv_projection
    }

    fn get_coord_matrix(&self) -> Mat4 {
        self.matrices().coord
    }

    fn get_inverted_matrix(&self) -> Mat4 {
        self.matrices().inverted
    }

    // MARK: - Dimensions

    pub fn get_size(&self) -> Size {
        self.size
    }

    pub fn set_size(&mut self, size: Size) {
        if self.size != size {
            self.size = size;
            self.invalidate_matrices();
        }
    }

    pub fn get_frustum_offset(&self) -> EdgeInsets {
        self.frustum_offset
    }

    pub fn set_frustum_offset(&mut self, frustum_offset: &EdgeInsets) {
        if self.frustum_offset != *frustum_offset {
            self.frustum_offset = *frustum_offset;
            self.invalidate_matrices();
        }
    }

    // MARK: - North Orientation

    pub fn get_north_orientation(&self) -> NorthOrientation {
        self.orientation
    }

    pub fn set_north_orientation(&mut self, val: NorthOrientation) {
        if self.orientation != val {
            self.orientation = val;
            self.invalidate_matrices();
        }
    }

    pub fn get_north_orientation_angle(&self) -> f64 {
        let mut angle_orientation = 0.0;
        if self.orientation == NorthOrientation::Rightwards {
            angle_orientation += PI / 2.0;
        } else if self.orientation == NorthOrientation::Downwards {
            angle_orientation += PI;
        } else if self.orientation == NorthOrientation::Leftwards {
            angle_orientation -= PI / 2.0;
        }
        angle_orientation
    }

    // MARK: - Constrain mode

    pub fn get_constrain_mode(&self) -> ConstrainMode {
        self.constrain_mode
    }

    pub fn set_constrain_mode(&mut self, val: ConstrainMode) {
        if self.constrain_mode != val {
            self.constrain_mode = val;
            self.invalidate_matrices();
        }
    }

    // MARK: - ViewportMode

    pub fn get_viewport_mode(&self) -> ViewportMode {
        self.viewport_mode
    }

    pub fn set_viewport_mode(&mut self, val: ViewportMode) {
        if self.viewport_mode != val {
            self.viewport_mode = val;
            self.invalidate_matrices();
        }
    }

    // MARK: - Camera options

    pub fn get_camera_options(&self, padding: Option<EdgeInsets>) -> CameraOptions {
        let camera = MapCamera { padding: padding.unwrap_or(self.edge_insets), ..self.camera() };
        CameraOptions::from(camera)
    }

    /// The camera. The globe keeps the center on the world copy nearest the
    /// previous one, which its coordinate conversions do not depend on, and
    /// reports it wrapped. Mercator's conversions use the copy the transform
    /// is on, and bounds across the antimeridian keep the center in their
    /// range, past 180 degrees: both report the center as it is.
    pub fn camera(&self) -> MapCamera {
        let wrap_mode = if self.is_globe_rendering() && self.bounds == LatLngBounds::default() {
            WrapMode::Wrapped
        } else {
            WrapMode::Unwrapped
        };
        MapCamera {
            center: self.get_lat_lng(wrap_mode),
            center_altitude: self.get_center_altitude(),
            padding: self.edge_insets,
            zoom: self.get_zoom(),
            bearing: mln_math::rad2deg(-self.bearing),
            pitch: mln_math::rad2deg(self.pitch),
            roll: mln_math::rad2deg(self.roll),
            fov: mln_math::rad2deg(self.fov),
        }
    }

    // MARK: - EdgeInsets

    pub fn get_edge_insets(&self) -> EdgeInsets {
        self.edge_insets
    }

    pub fn set_edge_insets(&mut self, val: &EdgeInsets) {
        if self.edge_insets != *val {
            self.edge_insets = *val;
            self.invalidate_matrices();
        }
    }

    // MARK: - Position

    /// `getLatLng(LatLng::WrapMode = LatLng::Unwrapped)`.
    pub fn get_lat_lng(&self, wrap_mode: WrapMode) -> LatLng {
        lat_lng(mln_math::rad2deg(2.0 * (self.y / self.cc).exp().atan() - 0.5 * PI), -self.x / self.bc, wrap_mode)
    }

    /// `latLngNearCenter(latLng)`: the world copy of a location nearest the
    /// center, which bounds can keep past the antimeridian.
    pub fn lat_lng_near_center(&self, location: &LatLng) -> LatLng {
        let center = self.get_lat_lng(WrapMode::Unwrapped);
        let wrapped_center = center.wrapped();
        let mut nearest = location.wrapped();
        nearest.unwrap_for_shortest_path(&wrapped_center);
        lat_lng(
            nearest.latitude(),
            nearest.longitude() + center.longitude() - wrapped_center.longitude(),
            WrapMode::Unwrapped,
        )
    }

    pub fn get_center_altitude(&self) -> f64 {
        self.z
            * Projection::get_meters_per_pixel_at_latitude(
                self.get_lat_lng(WrapMode::Unwrapped).latitude(),
                self.get_zoom(),
            )
    }

    pub fn pixel_x(&self) -> f64 {
        let center = (f64::from(self.size.width) - Projection::world_size(self.scale)) / 2.0;
        center + self.x
    }

    pub fn pixel_y(&self) -> f64 {
        let center = (f64::from(self.size.height) - Projection::world_size(self.scale)) / 2.0;
        center + self.y
    }

    // MARK: - Zoom

    pub fn get_zoom(&self) -> f64 {
        self.scale_zoom(self.scale)
    }

    /// Lowest zoom allowed with the map centered at this latitude: the globe
    /// lets the zoom fall below `get_min_zoom()` toward the poles by the
    /// anchored-zoom adjustment, so the planet keeps its size on screen.
    pub fn get_min_zoom_at_latitude(&self, latitude: f64) -> f64 {
        self.get_min_zoom_at_latitude_with(latitude, self.is_globe_rendering())
    }

    /// `getMinZoomAtLatitude(double latitude, bool globe)`: the same for a
    /// camera on the globe or not, whatever the current projection.
    pub fn get_min_zoom_at_latitude_with(&self, latitude: f64, globe: bool) -> f64 {
        if !globe {
            return self.get_min_zoom();
        }
        // The globe's constraint never raises the scale, whichever projection
        // the map is in now.
        self.scale_zoom(self.min_scale) + VerticalPerspectiveProjection::zoom_adjustment(0.0, latitude)
    }

    /// The center `constrain` would settle on for this one at this zoom: on
    /// the globe the zoom decides how close to a pole the center can be.
    pub fn constrained_center(&self, lat_lng: &LatLng, zoom: f64) -> LatLng {
        if self.constrain_mode == ConstrainMode::None || !self.is_globe_rendering() {
            return *lat_lng;
        }
        let max_latitude = VerticalPerspectiveProjection::max_latitude(
            self,
            mln_math::clamp_f64(zoom, self.get_min_zoom_at_latitude(lat_lng.latitude()), self.get_max_zoom()),
        );
        self::lat_lng(
            mln_math::clamp_f64(lat_lng.latitude(), -max_latitude, max_latitude),
            lat_lng.longitude(),
            WrapMode::Unwrapped,
        )
    }

    /// `static_cast<uint8_t>(std::max(0.0, getZoom()))`.
    pub fn get_integer_zoom(&self) -> u8 {
        std_max(0.0, self.get_zoom()) as u8
    }

    pub fn get_zoom_fraction(&self) -> f64 {
        self.get_zoom() - f64::from(self.get_integer_zoom())
    }

    // MARK: - Bounds

    pub fn set_lat_lng_bounds(&mut self, bounds: LatLngBounds) {
        if bounds != self.bounds {
            self.bounds = bounds;
            let lat_lng = self.get_lat_lng(WrapMode::Unwrapped);
            let zoom = self.get_zoom();
            self.set_lat_lng_zoom(&lat_lng, zoom);
        }
    }

    pub fn get_lat_lng_bounds(&self) -> LatLngBounds {
        self.bounds
    }

    pub fn set_min_zoom(&mut self, min_zoom: f64) {
        if min_zoom <= self.get_max_zoom() {
            self.min_scale = self.zoom_scale(mln_math::clamp_f64(min_zoom, constants::MIN_ZOOM, constants::MAX_ZOOM));
        }
    }

    pub fn get_min_zoom(&self) -> f64 {
        self.scale_zoom(self.constrained_min_scale())
    }

    fn constrained_min_scale(&self) -> f64 {
        let mut test_scale = self.min_scale;
        let mut unused_x = self.x;
        let mut unused_y = self.y;
        self.constrain(&mut test_scale, &mut unused_x, &mut unused_y);
        test_scale
    }

    pub fn set_max_zoom(&mut self, max_zoom: f64) {
        if max_zoom >= self.get_min_zoom() {
            self.max_scale = self.zoom_scale(mln_math::clamp_f64(max_zoom, constants::MIN_ZOOM, constants::MAX_ZOOM));
        }
    }

    pub fn get_max_zoom(&self) -> f64 {
        self.scale_zoom(self.max_scale)
    }

    pub fn set_min_pitch(&mut self, pitch: f64) {
        if pitch <= self.max_pitch {
            self.min_pitch = mln_math::clamp_f64(pitch, constants::PITCH_MIN, self.max_pitch);
        } else {
            Log::warning(Event::General, "Trying to set minimum pitch to larger than maximum pitch, no changes made.");
        }
    }

    pub fn get_min_pitch(&self) -> f64 {
        self.min_pitch
    }

    pub fn set_max_pitch(&mut self, pitch: f64) {
        if pitch >= self.min_pitch {
            self.max_pitch = mln_math::clamp_f64(pitch, self.min_pitch, PITCH_MAX);
        } else {
            Log::warning(Event::General, "Trying to set maximum pitch to smaller than minimum pitch, no changes made.");
        }
    }

    pub fn get_max_pitch(&self) -> f64 {
        self.max_pitch
    }

    pub fn get_min_field_of_view(&self) -> f64 {
        self.min_fov
    }

    pub fn get_max_field_of_view(&self) -> f64 {
        self.max_fov
    }

    // MARK: - Scale

    pub fn get_scale(&self) -> f64 {
        self.scale
    }

    pub fn set_scale(&mut self, val: f64) {
        if self.scale != val {
            self.scale = val;
            self.invalidate_matrices();
        }
    }

    // MARK: - Positions

    pub fn get_x(&self) -> f64 {
        self.x
    }

    pub fn set_x(&mut self, val: f64) {
        if self.x != val {
            self.x = val;
            self.invalidate_matrices();
        }
    }

    pub fn get_y(&self) -> f64 {
        self.y
    }

    pub fn set_y(&mut self, val: f64) {
        if self.y != val {
            self.y = val;
            self.invalidate_matrices();
        }
    }

    pub fn get_z(&self) -> f64 {
        self.z
    }

    pub fn set_z(&mut self, val: f64) {
        if self.z != val {
            self.z = val;
            self.invalidate_matrices();
        }
    }

    // MARK: - Rotation

    pub fn get_bearing(&self) -> f64 {
        self.bearing
    }

    pub fn set_bearing(&mut self, val: f64) {
        if self.bearing != val {
            self.bearing = val;
            self.invalidate_matrices();
        }
    }

    /// Returns float, as C++ does.
    pub fn get_field_of_view(&self) -> f32 {
        self.fov as f32
    }

    pub fn set_field_of_view(&mut self, val: f64) {
        if self.fov != val {
            self.fov = val;
            self.invalidate_matrices();
        }
    }

    pub fn get_roll(&self) -> f64 {
        self.roll
    }

    pub fn set_roll(&mut self, val: f64) {
        if self.roll != val {
            self.roll = val;
            self.invalidate_matrices();
        }
    }

    /// Returns float, as C++ does.
    pub fn get_camera_to_center_distance(&self) -> f32 {
        (0.5 * f64::from(self.size.height) / (self.fov / 2.0).tan()) as f32
    }

    pub fn get_pitch(&self) -> f64 {
        self.pitch
    }

    pub fn set_pitch(&mut self, val: f64) {
        if self.pitch != val {
            self.pitch = val;
            self.invalidate_matrices();
        }
    }

    pub fn get_x_skew(&self) -> f64 {
        self.x_skew
    }

    pub fn set_x_skew(&mut self, val: f64) {
        if self.x_skew != val {
            self.x_skew = val;
            self.invalidate_matrices();
        }
    }

    pub fn get_y_skew(&self) -> f64 {
        self.y_skew
    }

    pub fn set_y_skew(&mut self, val: f64) {
        if self.y_skew != val {
            self.y_skew = val;
            self.invalidate_matrices();
        }
    }

    pub fn get_axonometric(&self) -> bool {
        self.axonometric
    }

    pub fn set_axonometric(&mut self, val: bool) {
        if self.axonometric != val {
            self.axonometric = val;
            self.invalidate_matrices();
        }
    }

    // MARK: - State

    pub fn is_changing(&self) -> bool {
        self.rotating || self.scaling || self.panning || self.gesture_in_progress
    }

    pub fn is_rotating(&self) -> bool {
        self.rotating
    }

    pub fn set_rotating_in_progress(&mut self, val: bool) {
        self.rotating = val;
    }

    pub fn is_scaling(&self) -> bool {
        self.scaling
    }

    pub fn set_scaling_in_progress(&mut self, val: bool) {
        self.scaling = val;
    }

    pub fn is_panning(&self) -> bool {
        self.panning
    }

    pub fn set_panning_in_progress(&mut self, val: bool) {
        self.panning = val;
    }

    pub fn is_gesture_in_progress(&self) -> bool {
        self.gesture_in_progress
    }

    pub fn set_gesture_in_progress(&mut self, val: bool) {
        self.gesture_in_progress = val;
    }

    // MARK: - Projection

    pub fn zoom_scale(&self, zoom: f64) -> f64 {
        round_for_accuracy(2.0f64.powf(zoom))
    }

    pub fn scale_zoom(&self, s: f64) -> f64 {
        round_for_accuracy(s.log2())
    }

    /// Toward the poles the globe goes below the minimum scale;
    /// `set_lat_lng_zoom` holds it to the floor there.
    pub fn valid(&self) -> bool {
        !self.size.is_empty()
            && (if self.is_globe_rendering() { self.scale > 0.0 } else { self.scale >= self.min_scale })
            && self.scale <= self.max_scale
    }

    pub fn lat_lng_to_screen_coordinate(&self, lat_lng: &LatLng) -> ScreenCoordinate {
        let mut p: Vec4 = [0.0; 4];
        self.lat_lng_to_screen_coordinate_with(lat_lng, &mut p)
    }

    /// `latLngToScreenCoordinate(const LatLng&, vec4&)`.
    pub fn lat_lng_to_screen_coordinate_with(&self, lat_lng: &LatLng, p: &mut Vec4) -> ScreenCoordinate {
        if self.size.is_empty() {
            return ScreenCoordinate::new(0.0, 0.0);
        }

        if self.is_globe_rendering() {
            return VerticalPerspectiveProjection::lat_lng_to_screen_coordinate(self, lat_lng, p);
        }

        let pt = self.projection.project(lat_lng, self.scale);
        let pt = Point::new(pt.x / TILE_SIZE_D, pt.y / TILE_SIZE_D);
        let c: Vec4 = [pt.x, pt.y, 0.0, 1.0];
        matrix::transform_mat4(p, &c, &self.get_coord_matrix());
        ScreenCoordinate::new(p[0] / p[3], f64::from(self.size.height) - p[1] / p[3])
    }

    /// Whether the globe hides a location from the camera; a Mercator map
    /// hides nothing.
    pub fn is_location_occluded(&self, lat_lng: &LatLng) -> bool {
        if self.size.is_empty() || !self.is_globe_rendering() {
            return false;
        }
        let surface = VerticalPerspectiveProjection::surface_vector(lat_lng);
        let plane = self.get_globe_clipping_plane();
        plane[0] * surface[0] + plane[1] * surface[1] + plane[2] * surface[2] + plane[3] < 0.0
    }

    /// Implements mapbox-gl-js `pointCoordinate()` : `MercatorCoordinate`.
    pub fn screen_coordinate_to_tile_coordinate(&self, point: &ScreenCoordinate, at_zoom: u8) -> TileCoordinate {
        if self.size.is_empty() {
            return TileCoordinate { p: Point::new(0.0, 0.0), z: 0.0 };
        }

        if self.is_globe_rendering() {
            return TileCoordinate::from_lat_lng(
                f64::from(at_zoom),
                &VerticalPerspectiveProjection::screen_coordinate_to_lat_lng(self, point, WrapMode::Unwrapped),
            );
        }

        self.mercator_screen_coordinate_to_tile_coordinate(point, at_zoom)
    }

    fn mercator_screen_coordinate_to_tile_coordinate(&self, point: &ScreenCoordinate, at_zoom: u8) -> TileCoordinate {
        let target_z: f32 = 0.0;

        let flipped_y = f64::from(self.size.height) - point.y;

        // since we don't know the correct projected z value for the point,
        // unproject two points to get a line and then find the point on that
        // line with z=0

        let mut coord0: Vec4 = [0.0; 4];
        let mut coord1: Vec4 = [0.0; 4];
        let point0: Vec4 = [point.x, flipped_y, 0.0, 1.0];
        let point1: Vec4 = [point.x, flipped_y, 1.0, 1.0];
        let inverted = self.get_inverted_matrix();
        matrix::transform_mat4(&mut coord0, &point0, &inverted);
        matrix::transform_mat4(&mut coord1, &point1, &inverted);

        let w0 = coord0[3];
        let w1 = coord1[3];

        let p0 = Point::new(coord0[0] / w0, coord0[1] / w0);
        let p1 = Point::new(coord1[0] / w1, coord1[1] / w1);

        let z0 = coord0[2] / w0;
        let z1 = coord1[2] / w1;
        let t = if z0 <= z1 { 0.0 } else { (f64::from(target_z) - z0) / (z1 - z0) };

        let p = interpolate(&p0, &p1, t);
        // C++ `static_cast<double>(1 << atZoom)`: an int shift.
        let factor = f64::from(1i32.wrapping_shl(u32::from(at_zoom)));
        TileCoordinate { p: Point::new(p.x / self.scale * factor, p.y / self.scale * factor), z: f64::from(at_zoom) }
    }

    /// `screenCoordinateToLatLng(const ScreenCoordinate&, LatLng::WrapMode = LatLng::Unwrapped)`.
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// NaN or infinite point.
    pub fn screen_coordinate_to_lat_lng(&self, point: &ScreenCoordinate, wrap_mode: WrapMode) -> Result<LatLng, Error> {
        // Without a size the point is Mercator's, as the tile coordinate is.
        if self.is_globe_rendering() && !self.size.is_empty() {
            if point.x.is_nan() || point.y.is_nan() {
                return Err(Error::invalid_argument("screen coordinate must not be NaN"));
            }
            if !point.x.is_finite() || !point.y.is_finite() {
                return Err(Error::invalid_argument("screen coordinate must not be infinite"));
            }
            return Ok(VerticalPerspectiveProjection::screen_coordinate_to_lat_lng(self, point, wrap_mode));
        }
        let coord = self.screen_coordinate_to_tile_coordinate(point, 0);
        self.projection.unproject(&coord.p, 1.0 / TILE_SIZE_D, wrap_mode)
    }

    fn coordinate_point_matrix(&self, proj_matrix: &Mat4) -> Mat4 {
        let mut proj = *proj_matrix;
        let a = proj;
        matrix::scale(&mut proj, &a, TILE_SIZE_D, TILE_SIZE_D, 1.0);
        let b = proj;
        matrix::multiply(&mut proj, &self.get_pixel_matrix(), &b);
        proj
    }

    fn get_pixel_matrix(&self) -> Mat4 {
        let mut m: Mat4 = [0.0; 16];
        matrix::identity(&mut m);
        let a = m;
        matrix::scale(&mut m, &a, f64::from(self.size.width) / 2.0, -f64::from(self.size.height) / 2.0, 1.0);
        let a = m;
        matrix::translate(&mut m, &a, 1.0, -1.0, 0.0);
        m
    }

    // MARK: - (private helper functions)

    fn rotated_north(&self) -> bool {
        self.orientation == NorthOrientation::Leftwards || self.orientation == NorthOrientation::Rightwards
    }

    /// `constrainScreen(double& scale_, double& lat, double& lon)`.
    pub fn constrain_screen(&self, scale: &mut f64, lat: &mut f64, lon: &mut f64) -> bool {
        if self.constrain_mode == ConstrainMode::Screen {
            let mut zoom = self.scale_zoom(*scale);
            let mut options = CameraOptions::default();
            self.constrain_camera_and_zoom_to_bounds(&mut options, &mut zoom);

            *scale = self.zoom_scale(zoom);

            if let Some(center) = options.center {
                *lat = center.latitude();
                *lon = center.longitude();

                return true;
            }
        }
        false
    }

    /// `constrain(double& scale, double& x, double& y)`.
    pub fn constrain(&self, scale: &mut f64, x: &mut f64, y: &mut f64) {
        if self.constrain_mode == ConstrainMode::None || self.constrain_mode == ConstrainMode::Screen {
            return;
        }

        // The globe has no off-world area: the center goes past the Mercator
        // range toward a pole as far as the zoom allows.
        if self.is_globe_rendering() {
            let max_latitude = VerticalPerspectiveProjection::max_latitude(self, self.scale_zoom(*scale));
            let max_y = Projection::world_size(*scale) / M2PI * mln_math::asinh(mln_math::deg2rad(max_latitude).tan());
            *y = mln_math::clamp_f64(*y, -max_y, max_y);
            return;
        }

        let width = f64::from(self.size.width);
        let height = f64::from(self.size.height);

        // Constrain scale to avoid zooming out far enough to show off-world
        // areas on the Y axis.
        let ratio_y = (if self.rotated_north() { width } else { height }) / TILE_SIZE_D;
        // util::max: fmax
        *scale = mln_math::max_f64(*scale, ratio_y);

        // Constrain min/max pan to avoid showing off-world areas on the Y axis.
        let max_y = (*scale * TILE_SIZE_D - (if self.rotated_north() { width } else { height })) / 2.0;
        *y = std_max(-max_y, std_min(*y, max_y));

        if self.constrain_mode == ConstrainMode::WidthAndHeight {
            // Constrain min/max pan to avoid showing off-world areas on the X axis.
            let max_x = (*scale * TILE_SIZE_D - (if self.rotated_north() { height } else { width })) / 2.0;
            *x = std_max(-max_x, std_min(*x, max_x));
        }
    }

    pub fn constrain_camera_and_zoom_to_bounds(&self, requested_camera: &mut CameraOptions, requested_zoom: &mut f64) {
        if self.constrain_mode != ConstrainMode::Screen || self.get_lat_lng_bounds() == LatLngBounds::default() {
            return;
        }

        let mut center_lat_lng = self.get_lat_lng(WrapMode::Unwrapped);

        if let Some(center) = requested_camera.center {
            center_lat_lng = center;
        }

        let mut anchor_offset = Point::new(0.0, 0.0);
        let requested_scale = self.zoom_scale(*requested_zoom);

        // Since the transition calculations will include any specified anchor
        // in the result we need to do the same when testing if the requested
        // center and zoom is outside the bounds or not.
        if let Some(mut anchor) = requested_camera.anchor {
            anchor.y = f64::from(self.get_size().height) - anchor.y;
            // The screenCoordinateToLatLng function requires the matrices
            // inside the state to reflect the requested scale. So we create a
            // copy and set the requested zoom before the conversion. This will
            // give us the same result as the transition calculations.
            let mut state = self.clone();
            state.set_lat_lng_zoom(&self.get_lat_lng(WrapMode::Unwrapped), self.scale_zoom(requested_scale));
            // C++ throws std::domain_error out of the camera change for an
            // anchor that has no LatLng; the port ignores the anchor, as
            // Transform::startTransition does.
            if let (Ok(anchor_lat_lng), Ok(screen_lat_lng)) = (
                self.screen_coordinate_to_lat_lng(&anchor, WrapMode::Unwrapped),
                state.screen_coordinate_to_lat_lng(&anchor, WrapMode::Unwrapped),
            ) {
                let lat_lng_coord = Projection::project(&anchor_lat_lng, requested_scale);
                let anchor_coord = Projection::project(&screen_lat_lng, requested_scale);
                anchor_offset = Point::new(lat_lng_coord.x - anchor_coord.x, lat_lng_coord.y - anchor_coord.y);
            }
        }

        let current_bounds = self.get_lat_lng_bounds();
        let ne_bounds = Projection::project(&current_bounds.northeast(), requested_scale);
        let sw_bounds = Projection::project(&current_bounds.southwest(), requested_scale);
        let center = Projection::project(&center_lat_lng, requested_scale);
        let current_center = Projection::project(&self.get_lat_lng(WrapMode::Unwrapped), requested_scale);

        let min_y = ne_bounds.y;
        let max_y = sw_bounds.y;
        let min_x = sw_bounds.x;
        let max_x = ne_bounds.x;

        let start_x = center.x;
        let start_y = center.y;

        let mut result_x = start_x;
        let mut result_y = start_y;

        let screen_width = self.get_size().width;
        let screen_height = self.get_size().height;

        let h2 = f64::from(screen_height) / 2.0;
        if start_y - h2 + anchor_offset.y < min_y {
            result_y = min_y + h2;
        }
        if start_y + anchor_offset.y + h2 > max_y {
            result_y = max_y - h2;
        }

        let w2 = f64::from(screen_width) / 2.0;
        if start_x + anchor_offset.x - w2 < min_x {
            result_x = min_x + w2;
        }
        if start_x + anchor_offset.x + w2 > max_x {
            result_x = max_x - w2;
        }

        let mut scale_y = 0.0;
        if max_y - min_y < f64::from(screen_height) {
            scale_y = f64::from(screen_height) / (max_y - min_y);
            result_y = f64::midpoint(max_y, min_y);
        }

        let mut scale_x = 0.0;
        if max_x - min_x < f64::from(screen_width) {
            scale_x = f64::from(screen_width) / (max_x - min_x);
            result_x = f64::midpoint(max_x, min_x);
        }

        let max_scale = if scale_x > scale_y { scale_x } else { scale_y };

        // Max scale will be 1 when the screen is exactly the same size as the
        // max bounds in either the X or Y direction. To avoid numerical
        // instabilities we add small amount to the check to make sure we
        // don't try to scale when we don't actually need it.
        if max_scale > 1.000_001 {
            *requested_zoom += self.scale_zoom(max_scale);

            if scale_y > scale_x {
                // If we scaled the y direction we want the resulting x
                // position to be the same as the current x position.
                result_x = current_center.x;
            } else {
                // If we scaled the x direction we want the resulting y
                // position to be the same as the current y position.
                result_y = current_center.y;
            }

            // Since we changed the scale, we might display something outside
            // the bounds. When checking we need to take into consideration
            // that we just changed the scale, since the resultX and minX were
            // calculated with the requested scale, and not the scale we just
            // calculated to make sure we stay inside the bounds.
            if result_x * max_scale - w2 <= min_x * max_scale {
                result_x = min_x * max_scale + w2;
                result_x /= max_scale;
            } else if result_x * max_scale + w2 >= max_x * max_scale {
                result_x = max_x * max_scale - w2;
                result_x /= max_scale;
            }

            if result_y * max_scale - h2 <= min_y * max_scale {
                result_y = min_y * max_scale + h2;
                result_y /= max_scale;
            } else if result_y * max_scale + h2 >= max_y * max_scale {
                result_y = max_y * max_scale - h2;
                result_y /= max_scale;
            }
        }

        if result_x != start_x || result_y != start_y {
            // If we made changes just drop any anchor point
            requested_camera.anchor = None;
            // The clamped point is inside the bounds, so it has a LatLng.
            requested_camera.center =
                Projection::unproject(&Point::new(result_x, result_y), requested_scale, WrapMode::Unwrapped).ok();
        }
    }

    /// The lowest zoom `ConstrainMode::Screen` allows: the one at which the
    /// lat/lng bounds fill the screen, which
    /// [`constrain_camera_and_zoom_to_bounds`](Self::constrain_camera_and_zoom_to_bounds)
    /// raises lower zooms to. `None` in the other modes or without bounds.
    pub fn get_min_zoom_for_screen_bounds(&self) -> Option<f64> {
        if self.constrain_mode != ConstrainMode::Screen || self.bounds == LatLngBounds::default() {
            return None;
        }
        // The bounds' extent on screen grows with the scale alone, so they
        // fill the screen at the same zoom wherever the center is.
        let ne = Projection::project(&self.bounds.northeast(), 1.0);
        let sw = Projection::project(&self.bounds.southwest(), 1.0);
        let scale_x = f64::from(self.size.width) / (ne.x - sw.x);
        let scale_y = f64::from(self.size.height) / (sw.y - ne.y);
        Some(self.scale_zoom(std_max(scale_x, scale_y)))
    }

    /// Viewport center offset, from [size.width / 2, size.height / 2],
    /// defined by |edgeInsets| in screen coordinates, with top left origin.
    pub fn get_center_offset(&self) -> ScreenCoordinate {
        ScreenCoordinate::new(
            0.5 * (self.edge_insets.left - self.edge_insets.right),
            0.5 * (self.edge_insets.top - self.edge_insets.bottom),
        )
    }

    /// Recenter the map so that the given coordinate is located at the given
    /// point on screen.
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// NaN or infinite anchor.
    pub fn move_lat_lng(&mut self, lat_lng: &LatLng, anchor: &ScreenCoordinate) -> Result<(), Error> {
        if self.is_globe_rendering() {
            if let Some(center) = VerticalPerspectiveProjection::center_for_location_at_point(self, lat_lng, anchor) {
                let target = self.constrained_center(&center, self.get_zoom());
                let zoom = self.get_zoom()
                    + VerticalPerspectiveProjection::zoom_adjustment(
                        self.get_lat_lng(WrapMode::Unwrapped).latitude(),
                        target.latitude(),
                    );
                self.set_lat_lng_zoom(&target, zoom);
            }
            return Ok(());
        }
        let center_coord = Projection::project(&self.get_lat_lng(WrapMode::Unwrapped), self.scale);
        let lat_lng_coord = Projection::project(lat_lng, self.scale);
        let anchor_coord =
            Projection::project(&self.screen_coordinate_to_lat_lng(anchor, WrapMode::Unwrapped)?, self.scale);
        let center = Projection::unproject(
            &Point::new(
                center_coord.x + lat_lng_coord.x - anchor_coord.x,
                center_coord.y + lat_lng_coord.y - anchor_coord.y,
            ),
            self.scale,
            WrapMode::Unwrapped,
        )?;
        // Wrapped, as easeTo and GL JS's setLocationAtPoint leave it, unless
        // bounds keep it in their range.
        let target = if self.bounds == LatLngBounds::default() { center.wrapped() } else { center };
        let zoom = self.get_zoom();
        self.set_lat_lng_zoom(&target, zoom);
        Ok(())
    }

    pub fn set_lat_lng_zoom(&mut self, lat_lng: &LatLng, zoom: f64) {
        let mut requested = *lat_lng;
        // Globe centers arrive wrapped, and bounds across the antimeridian
        // reach past 180 degrees: take the copy of the longitude nearest the
        // bounds before constraining it.
        if self.is_globe_rendering() && self.bounds != LatLngBounds::default() {
            let turns = ((self.bounds.center().longitude() - lat_lng.longitude()) / constants::DEGREES_MAX).round();
            requested = self::lat_lng(
                lat_lng.latitude(),
                lat_lng.longitude() + turns * constants::DEGREES_MAX,
                WrapMode::Unwrapped,
            );
        }
        let mut constrained = self.bounds.constrain(&requested);
        // The globe's tile cover picks each tile's wrap from the center, so a
        // center that jumped a whole world at the antimeridian would re-key
        // every tile; keep it on the copy nearest the current one.
        if self.is_globe_rendering() && self.bounds == LatLngBounds::default() {
            let current = self.get_lat_lng(WrapMode::Unwrapped).longitude();
            if current.is_finite() {
                let turns = ((current - constrained.longitude()) / constants::DEGREES_MAX).round();
                constrained = self::lat_lng(
                    constrained.latitude(),
                    constrained.longitude() + turns * constants::DEGREES_MAX,
                    WrapMode::Unwrapped,
                );
            }
        }

        let min_scale = if self.is_globe_rendering() {
            self.zoom_scale(self.get_min_zoom_at_latitude(constrained.latitude()))
        } else {
            self.constrained_min_scale()
        };
        let new_scale = mln_math::clamp_f64(self.zoom_scale(zoom), min_scale, self.max_scale);
        let new_world_size = new_scale * TILE_SIZE_D;
        self.bc = new_world_size / constants::DEGREES_MAX;
        self.cc = new_world_size / M2PI;

        // The globe's center can be on a pole, which the clamped sine stops
        // short of.
        let latitude = mln_math::deg2rad(constrained.latitude());
        let m = 1.0 - 1e-15;
        let f = mln_math::clamp_f64(latitude.sin(), -m, m);

        let point = ScreenCoordinate::new(
            -constrained.longitude() * self.bc,
            if self.is_globe_rendering() {
                self.cc * mln_math::asinh(latitude.tan())
            } else {
                0.5 * self.cc * ((1.0 + f) / (1.0 - f)).ln()
            },
        );
        self.set_scale_point(new_scale, &point);
    }

    pub fn set_center_altitude(&mut self, alt_m: f64) {
        self.z = alt_m
            / Projection::get_meters_per_pixel_at_latitude(
                self.get_lat_lng(WrapMode::Unwrapped).latitude(),
                self.get_zoom(),
            );
        self.invalidate_matrices();
    }

    fn set_scale_point(&mut self, new_scale: f64, point: &ScreenCoordinate) {
        let mut constrained_scale = new_scale;
        let mut constrained_point = *point;
        self.constrain(&mut constrained_scale, &mut constrained_point.x, &mut constrained_point.y);

        self.scale = constrained_scale;
        self.x = constrained_point.x;
        self.y = constrained_point.y;
        self.bc = Projection::world_size(self.scale) / constants::DEGREES_MAX;
        self.cc = Projection::world_size(self.scale) / M2PI;
        self.invalidate_matrices();
    }

    pub fn get_camera_to_tile_distance(&self, tile_id: &UnwrappedTileID) -> f32 {
        let mut tile_projection_matrix: Mat4 = [0.0; 16];
        self.matrix_for(&mut tile_projection_matrix, tile_id);
        let b = tile_projection_matrix;
        matrix::multiply(&mut tile_projection_matrix, &self.get_projection_matrix(), &b);
        let tile_center: Vec4 = [TILE_SIZE_D / 2.0, TILE_SIZE_D / 2.0, 0.0, 1.0];
        let mut projected_center: Vec4 = [0.0; 4];
        matrix::transform_mat4(&mut projected_center, &tile_center, &tile_projection_matrix);
        projected_center[3] as f32
    }

    pub fn max_pitch_scale_factor(&self) -> f32 {
        if self.size.is_empty() {
            return 0.0;
        }
        // The globe pads queries with the Mercator factor, as GL JS does, from
        // the corner's unclamped Mercator position. C++ builds the point from
        // `static_cast<float>(height)`.
        let corner = self.mercator_screen_coordinate_to_tile_coordinate(
            &ScreenCoordinate::new(0.0, f64::from(self.get_size().height as f32)),
            0,
        );
        // A point on the screen has a LatLng unless the state holds NaN,
        // where C++ throws; the port renders without the pitch factor.
        let Ok(lat_lng) = self.projection.unproject(&corner.p, 1.0 / TILE_SIZE_D, WrapMode::Unwrapped) else {
            return 0.0;
        };

        let pt = if self.is_globe_rendering() {
            Point::new(corner.p.x * self.scale, corner.p.y * self.scale)
        } else {
            let pt = Projection::project(&lat_lng, self.scale);
            Point::new(pt.x / TILE_SIZE_D, pt.y / TILE_SIZE_D)
        };
        let p: Vec4 = [pt.x, pt.y, 0.0, 1.0];
        let mut top_point: Vec4 = [0.0; 4];
        matrix::transform_mat4(&mut top_point, &p, &self.get_coord_matrix());
        top_point[3] as f32 / self.get_camera_to_center_distance()
    }
}
