//! One view of a map: what its camera sees, with the matrices that draw it
//! and convert between the screen and the map.

use std::f64::consts::PI;

use geometry::Point;
use glam::dcamera::rh::proj::opengl::perspective;
use glam::{DMat4, DVec2, DVec3, DVec4};
use mln_style::constants::TILE_SIZE_D;
use mln_style::error::Error;
use mln_style::geo::{LatLng, WrapMode};
use mln_style::size::Size;
use mln_style::types::ProjectionDefinition;

use crate::camera::camera::FreeCameraOptions;
use crate::camera::camera_pose::CameraPose;
use crate::camera::constants::DEFAULT_FOV;
use crate::camera::map_view::{Camera, LogicalExtent, MapView, Viewport};
use crate::camera::mode::{ConstrainMode, ViewportMode};
use crate::camera::projection::{ProjectionData, mercator_tile_matrix};
use crate::camera::projection_mode::{AxonometricMode, ProjectionMode};
use crate::camera::tile_coordinate::TileCoordinate;
use crate::camera::vertical_perspective_projection::VerticalPerspectiveProjection;
pub use crate::geo::NorthOrientation;
use crate::geo::constants::M2PI;
use crate::geo::projection::Projection;
use crate::geo::{EdgeInsets, ScreenCoordinate};
use crate::tile::UnwrappedTileID;

/// The largest pitch the Mercator horizon is computed for, in radians. Past
/// it the distance to the horizon grows without bound, and with it the tiles
/// the view needs.
pub(crate) fn max_mercator_horizon_angle() -> f64 {
    mln_style::math::deg2rad(89.25)
}

/// `x` rounded to the nearest integer when it is within a billionth of it,
/// so that a zoom of 13.9999999 draws as 14.
fn round_for_accuracy(x: f64) -> f64 {
    let round_x = x.round();
    let diff = (round_x - x).abs();
    if diff < 1e-9 && diff > 0.0 { round_x } else { x }
}

/// The scale of `zoom`: 2 to its power.
#[must_use]
pub fn zoom_scale(zoom: f64) -> f64 {
    round_for_accuracy(2.0f64.powf(zoom))
}

/// The zoom of `scale`: its base-2 logarithm.
#[must_use]
pub fn scale_zoom(scale: f64) -> f64 {
    round_for_accuracy(scale.log2())
}

/// The map's offset from the world's center that puts `center` at the
/// center of a view at `scale`, in pixels: the negated center. The globe's
/// center can be on a pole, which Mercator's stops short of.
pub(crate) fn map_offset(center: &LatLng, scale: f64, globe: bool) -> (f64, f64) {
    let world_size = Projection::world_size(scale);
    let latitude = mln_style::math::deg2rad(center.latitude());
    let m = 1.0 - 1e-15;
    let f = mln_style::math::clamp_f64(latitude.sin(), -m, m);
    let x = -center.longitude() * (world_size / 360.0);
    let cc = world_size / M2PI;
    let y = if globe { cc * latitude.tan().asinh() } else { 0.5 * cc * ((1.0 + f) / (1.0 - f)).ln() };
    (x, y)
}

/// The fractional part of `x`, with the sign of `x`.
fn modf_fraction(x: f64) -> f64 {
    if x.is_infinite() {
        return 0.0f64.copysign(x);
    }
    (x - x.trunc()).copysign(x)
}

/// What a view shows, in the units its math works in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ViewParams {
    pub(crate) size: Size,
    pub(crate) frustum_offset: EdgeInsets,
    pub(crate) north_orientation: NorthOrientation,
    pub(crate) viewport_mode: ViewportMode,
    /// The constrain mode of the map the view comes from, which
    /// [`View::map_view`] reports.
    pub(crate) constrain_mode: ConstrainMode,
    /// Whether the center of the map the view comes from stands on its
    /// terrain, which [`View::map_view`] reports.
    pub(crate) center_clamped_to_ground: bool,
    /// Whether drags and zooms on the globe of the map the view comes from
    /// turn the bearing, which [`View::map_view`] reports.
    pub(crate) globe_free_bearing: bool,
    /// The style's projection, as it was last set.
    pub(crate) projection_definition: ProjectionDefinition,
    /// 0 is Mercator, 1 is the globe, in between is the transition.
    pub(crate) projection_transition: f64,
    pub(crate) padding: EdgeInsets,
    /// The map's offset from the world's center, in pixels at `scale`: the
    /// negated center.
    pub(crate) x: f64,
    pub(crate) y: f64,
    /// The center's altitude, in meters. It holds through changes of the
    /// zoom and the center; the matrices derive its pixels.
    pub(crate) center_altitude: f64,
    pub(crate) scale: f64,
    /// The rotation of the map, in radians counterclockwise: the negated
    /// bearing.
    pub(crate) angle: f64,
    pub(crate) pitch: f64,
    pub(crate) roll: f64,
    pub(crate) fov: f64,
    pub(crate) x_skew: f64,
    pub(crate) y_skew: f64,
    pub(crate) axonometric: bool,
    /// How [`View::camera`] reports the center.
    pub(crate) center_wrap: WrapMode,
    /// While terrain draws, the lowest ground near the center, in meters,
    /// exaggerated; the near and far planes then fit raised ground.
    pub(crate) terrain_min_elevation: Option<f64>,
}

impl ViewParams {
    pub(crate) fn new(constrain_mode: ConstrainMode, viewport_mode: ViewportMode) -> Self {
        ViewParams {
            size: Size::default(),
            frustum_offset: EdgeInsets::default(),
            north_orientation: NorthOrientation::Upwards,
            viewport_mode,
            constrain_mode,
            center_clamped_to_ground: true,
            globe_free_bearing: false,
            projection_definition: ProjectionDefinition::default(),
            projection_transition: 0.0,
            padding: EdgeInsets::default(),
            x: 0.0,
            y: 0.0,
            center_altitude: 0.0,
            scale: 1.0,
            angle: 0.0,
            pitch: 0.0,
            roll: 0.0,
            fov: DEFAULT_FOV,
            x_skew: 0.0,
            y_skew: 1.0,
            axonometric: false,
            center_wrap: WrapMode::Unwrapped,
            terrain_min_elevation: None,
        }
    }
}

/// The matrices of a view, zero while its size is empty or its projection
/// can't be inverted. The globe's are zero unless it renders the globe.
#[derive(Clone, Copy, Debug)]
struct Matrices {
    projection: DMat4,
    inv_projection: DMat4,
    coord: DMat4,
    /// The inverse of `coord` from the center of the map: it takes a screen
    /// point to its offset from the center, which keeps the precision that
    /// the whole world's coordinates would lose.
    inverted_from_center: DMat4,
    globe_view_projection: DMat4,
    inv_globe_view_projection: DMat4,
    globe_clipping_plane: DVec4,
    globe_camera_position: DVec3,
}

impl Matrices {
    const ZERO: Matrices = Matrices {
        projection: DMat4::ZERO,
        inv_projection: DMat4::ZERO,
        coord: DMat4::ZERO,
        inverted_from_center: DMat4::ZERO,
        globe_view_projection: DMat4::ZERO,
        inv_globe_view_projection: DMat4::ZERO,
        globe_clipping_plane: DVec4::ZERO,
        globe_camera_position: DVec3::ZERO,
    };
}

/// One view of a map: its camera, size and projection, and the matrices it
/// draws with. A view never changes: a map that moves its camera makes a new
/// one.
///
/// Angles are in radians. A view with an empty size, or a camera whose
/// projection can't be inverted, shows nothing.
#[derive(Clone, Debug)]
pub struct View {
    params: ViewParams,
    matrices: Matrices,
}

impl PartialEq for View {
    /// Two views are equal if they show the same: the matrices follow.
    fn eq(&self, other: &Self) -> bool {
        self.params == other.params
    }
}

impl Default for View {
    fn default() -> Self {
        View::new(ViewParams::new(ConstrainMode::HeightOnly, ViewportMode::Default))
    }
}

impl View {
    pub(crate) fn new(params: ViewParams) -> Self {
        let mut view = View { params, matrices: Matrices::ZERO };
        view.matrices = view.compute_matrices();
        view
    }

    pub(crate) fn params(&self) -> &ViewParams {
        &self.params
    }

    /// The matrices, from the view's parameters alone.
    fn compute_matrices(&self) -> Matrices {
        if self.params.size.is_empty() {
            return Matrices::ZERO;
        }
        let projection = self.projection_matrix_with(self.near_z(), false);
        let coord = self.coordinate_point_matrix(&projection);
        let from_center = coord * DMat4::from_translation(self.center_coordinate());
        let (Some(inv_projection), Some(inverted_from_center)) = (projection.try_inverse(), from_center.try_inverse())
        else {
            return Matrices::ZERO;
        };
        let mut matrices = Matrices { projection, inv_projection, coord, inverted_from_center, ..Matrices::ZERO };
        if self.is_globe() {
            let globe_radius = VerticalPerspectiveProjection::globe_radius_pixels(
                Projection::world_size(self.params.scale),
                self.center(WrapMode::Unwrapped).latitude(),
            );
            let globe_view_projection = VerticalPerspectiveProjection::globe_view_projection_matrix(self, globe_radius);
            matrices.globe_view_projection = globe_view_projection;
            matrices.inv_globe_view_projection = globe_view_projection.try_inverse().unwrap_or(DMat4::IDENTITY);
            matrices.globe_clipping_plane = VerticalPerspectiveProjection::clipping_plane(self, globe_radius);
            matrices.globe_camera_position = VerticalPerspectiveProjection::camera_position(self, globe_radius);
        }
        matrices
    }

    // MARK: - Dimensions

    /// The size, in logical pixels.
    #[must_use]
    pub fn size(&self) -> Size {
        self.params.size
    }

    /// Insets that move the vanishing point off the center of the view.
    #[must_use]
    pub(crate) fn frustum_offset(&self) -> EdgeInsets {
        self.params.frustum_offset
    }

    /// Insets from the view's edges. The camera centers within the rest.
    #[must_use]
    pub(crate) fn padding(&self) -> EdgeInsets {
        self.params.padding
    }

    #[must_use]
    pub(crate) fn north_orientation(&self) -> NorthOrientation {
        self.params.north_orientation
    }

    /// The rotation that the north orientation adds, in radians.
    #[must_use]
    pub(crate) fn north_orientation_angle(&self) -> f64 {
        match self.params.north_orientation {
            NorthOrientation::Upwards => 0.0,
            NorthOrientation::Rightwards => PI / 2.0,
            NorthOrientation::Downwards => PI,
            NorthOrientation::Leftwards => -PI / 2.0,
        }
    }

    #[must_use]
    pub(crate) fn viewport_mode(&self) -> ViewportMode {
        self.params.viewport_mode
    }

    /// Whether the map's center stands on its terrain
    /// ([`Viewport::center_clamped_to_ground`]).
    #[must_use]
    pub(crate) fn center_clamped_to_ground(&self) -> bool {
        self.params.center_clamped_to_ground
    }

    /// Whether drags and zooms on the globe turn the bearing
    /// ([`Viewport::globe_free_bearing`]).
    #[must_use]
    pub(crate) fn globe_free_bearing(&self) -> bool {
        self.params.globe_free_bearing
    }

    /// The center's offset from the middle of the view that the padding
    /// makes, in pixels from the top left.
    pub(crate) fn center_offset(&self) -> ScreenCoordinate {
        let padding = &self.params.padding;
        ScreenCoordinate::new(0.5 * (padding.left - padding.right), 0.5 * (padding.top - padding.bottom))
    }

    // MARK: - Camera

    /// The center. `WrapMode::Unwrapped` keeps the copy of the world that
    /// the camera is in.
    #[must_use]
    pub fn center(&self, wrap_mode: WrapMode) -> LatLng {
        let world_size = Projection::world_size(self.params.scale);
        LatLng::saturating(
            mln_style::math::rad2deg(2.0 * (self.params.y / (world_size / M2PI)).exp().atan() - 0.5 * PI),
            -self.params.x / (world_size / 360.0),
            wrap_mode,
        )
    }

    /// The world copy of `location` nearest the center, which bounds can keep
    /// past the antimeridian.
    #[must_use]
    pub fn lat_lng_near_center(&self, location: &LatLng) -> LatLng {
        let center = self.center(WrapMode::Unwrapped);
        let wrapped_center = center.wrapped();
        let mut nearest = location.wrapped();
        nearest.unwrap_for_shortest_path(&wrapped_center);
        LatLng::saturating(
            nearest.latitude(),
            nearest.longitude() + center.longitude() - wrapped_center.longitude(),
            WrapMode::Unwrapped,
        )
    }

    /// The center's altitude, in meters.
    #[must_use]
    pub fn center_altitude(&self) -> f64 {
        self.params.center_altitude
    }

    /// Meters per pixel at the zoom, as at the center (GL JS's
    /// `pixelsPerMeter`, inverted): the scale of every height the matrices
    /// place, so that a point at the center's altitude draws at the center.
    pub(crate) fn meters_per_pixel(&self) -> f64 {
        Projection::meters_per_pixel_at_latitude(self.center(WrapMode::Unwrapped).latitude(), self.zoom())
    }

    /// The center's altitude in pixels at the zoom.
    fn center_altitude_pixels(&self) -> f64 {
        // A center at sea level, as on most maps, spares the conversion's
        // transcendental functions on every new view.
        if self.params.center_altitude == 0.0 {
            return self.params.center_altitude;
        }
        self.params.center_altitude / self.meters_per_pixel()
    }

    #[must_use]
    pub fn zoom(&self) -> f64 {
        scale_zoom(self.params.scale)
    }

    /// The zoom, rounded down and clamped to the `u8` range.
    #[must_use]
    pub fn integer_zoom(&self) -> u8 {
        f64::max(0.0, self.zoom()) as u8
    }

    /// The zoom's fractional part.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn zoom_fraction(&self) -> f64 {
        self.zoom() - f64::from(self.integer_zoom())
    }

    /// 2 to the power of the zoom.
    #[must_use]
    pub(crate) fn scale(&self) -> f64 {
        self.params.scale
    }

    /// The map's offset from the world's center along x, in pixels at the
    /// scale: the negated center.
    #[must_use]
    pub(crate) fn x(&self) -> f64 {
        self.params.x
    }

    /// The map's offset from the world's center along y.
    #[must_use]
    pub(crate) fn y(&self) -> f64 {
        self.params.y
    }

    /// The map's rotation, counterclockwise: the negated bearing.
    #[must_use]
    pub fn angle(&self) -> f64 {
        self.params.angle
    }

    /// The tilt toward the horizon. 0 looks straight down.
    #[must_use]
    pub fn pitch(&self) -> f64 {
        self.params.pitch
    }

    #[must_use]
    pub fn roll(&self) -> f64 {
        self.params.roll
    }

    /// The vertical field of view.
    #[must_use]
    pub(crate) fn fov(&self) -> f64 {
        self.params.fov
    }

    /// The field of view that the perspective projections use: [`fov`]
    /// rounded to `f32`, the precision of the camera that drew the render
    /// tests' expected images. At full precision, edges land a fraction of a
    /// pixel away from theirs.
    ///
    /// [`fov`]: Self::fov
    pub(crate) fn perspective_fov(&self) -> f64 {
        f64::from(self.params.fov as f32)
    }

    /// The distance from the camera to the center, in pixels.
    #[must_use]
    pub fn camera_to_center_distance(&self) -> f64 {
        0.5 * f64::from(self.params.size.height) / (self.params.fov / 2.0).tan()
    }

    /// GL JS's `getMercatorHorizon`: how far above the center of the view,
    /// in pixels, the map ends and the sky starts. It is 0.85 of the way to
    /// the flat map's geometric horizon, and never past 89.25 degrees from
    /// straight down; at a pitch past that it is negative.
    #[must_use]
    pub(crate) fn mercator_horizon(&self) -> f64 {
        let pitch = self.params.pitch;
        self.camera_to_center_distance()
            * f64::min((PI / 2.0 - pitch).tan() * 0.85, (max_mercator_horizon_angle() - pitch).tan())
    }

    #[must_use]
    pub(crate) fn projection_mode(&self) -> ProjectionMode {
        ProjectionMode {
            axonometric: Some(AxonometricMode::from(self.params.axonometric)),
            x_skew: Some(self.params.x_skew),
            y_skew: Some(self.params.y_skew),
        }
    }

    /// The camera, in degrees, as the map reports it.
    #[must_use]
    pub(crate) fn camera(&self) -> Camera {
        Camera {
            center: self.center(self.params.center_wrap),
            center_altitude: self.center_altitude(),
            padding: self.params.padding,
            zoom: self.zoom(),
            bearing: mln_style::math::rad2deg(-self.params.angle),
            pitch: mln_style::math::rad2deg(self.params.pitch),
            roll: mln_style::math::rad2deg(self.params.roll),
            fov: mln_style::math::rad2deg(self.params.fov),
        }
    }

    /// The view as its public description, at `scale_factor` physical pixels
    /// in each logical pixel.
    #[must_use]
    pub fn map_view(&self, scale_factor: f64) -> MapView {
        MapView {
            camera: self.camera(),
            extent: LogicalExtent { size: self.params.size, scale_factor },
            projection_mode: self.projection_mode(),
            viewport: Viewport {
                north_orientation: self.params.north_orientation,
                constrain_mode: self.params.constrain_mode,
                viewport_mode: self.params.viewport_mode,
                center_clamped_to_ground: self.params.center_clamped_to_ground,
                globe_free_bearing: self.params.globe_free_bearing,
                frustum_offset: self.params.frustum_offset,
            },
            projection: self.params.projection_definition,
        }
    }

    /// Whether the view shows anything: it has a size and a scale.
    #[must_use]
    pub(crate) fn is_valid(&self) -> bool {
        !self.params.size.is_empty() && self.params.scale.is_finite() && self.params.scale > 0.0
    }

    /// The camera's position and orientation, or the default pose while the
    /// view isn't valid.
    pub(crate) fn camera_pose(&self) -> CameraPose {
        if !self.is_valid() {
            return CameraPose::default();
        }
        let world_size = Projection::world_size(self.params.scale);
        // x and y are the map's position, the camera's negated, from the
        // middle of the world.
        let center = DVec3::new(
            0.5 * world_size - self.params.x,
            0.5 * world_size - self.params.y,
            self.center_altitude_pixels(),
        );
        // Orient the camera, then move it back from the center along its line
        // of sight.
        let mut camera = CameraPose::from_angles(DVec3::ZERO, self.params.roll, self.params.pitch, self.params.angle);
        camera.position = (center - camera.forward() * self.camera_to_center_distance()) / world_size;
        camera
    }

    /// The camera's position and orientation.
    #[must_use]
    pub(crate) fn free_camera_options(&self) -> FreeCameraOptions {
        let camera = self.camera_pose();
        FreeCameraOptions { position: Some(camera.position), orientation: Some(camera.orientation) }
    }

    /// Where the camera's eye is: its Web Mercator point at zoom 0 (from 0
    /// to 1 across, y south) and its altitude in meters, of the meters the
    /// view places heights in ([`center_altitude`](Self::center_altitude)).
    /// On the globe it is the point under the eye, in the world copy nearest
    /// the center, with y past 0 or 1 over a polar cap, and the eye's height
    /// above the sea-level sphere.
    #[must_use]
    pub(crate) fn eye_position(&self) -> DVec3 {
        if self.is_globe() && self.is_valid() {
            let eye = self.matrices.globe_camera_position;
            return VerticalPerspectiveProjection::sphere_point_to_world(self, eye).unwrap_or(eye);
        }
        let position = self.camera_pose().position;
        let world_size = Projection::world_size(self.params.scale);
        DVec3::new(position.x, position.y, position.z * world_size * self.meters_per_pixel())
    }

    /// A 3 by 3 grid over the projection matrix's near plane, its corners,
    /// the middles of its edges and its center, in the units of
    /// [`eye_position`](Self::eye_position): the renderer drops what is
    /// nearer, so ground above a point shows as a hole into it. On the globe
    /// it is the globe matrix's near plane. All are the eye while the view
    /// shows nothing.
    #[must_use]
    pub(crate) fn near_plane_points(&self) -> [DVec3; 9] {
        let eye = self.eye_position();
        if self.matrices.projection == DMat4::ZERO {
            return [eye; 9];
        }
        let clip = |index: usize| DVec3::new((index % 3) as f64 - 1.0, (index / 3) as f64 - 1.0, -1.0);
        if self.is_globe() {
            let inverse = self.matrices.inv_globe_view_projection;
            return std::array::from_fn(|index| {
                let point = inverse.project_point3(clip(index));
                VerticalPerspectiveProjection::sphere_point_to_world(self, point).unwrap_or(eye)
            });
        }
        let world_size = Projection::world_size(self.params.scale);
        std::array::from_fn(|index| {
            let point = self.matrices.inv_projection.project_point3(clip(index));
            DVec3::new(point.x / world_size, point.y / world_size, point.z)
        })
    }

    // MARK: - Projection

    /// 0 is Mercator, 1 is the globe, in between is the transition.
    #[must_use]
    pub(crate) fn projection_transition(&self) -> f64 {
        self.params.projection_transition
    }

    /// Whether the view draws the globe, also while it hands over to or from
    /// Mercator.
    #[must_use]
    pub fn is_globe(&self) -> bool {
        self.params.projection_transition > 0.0
    }

    /// The globe while [`is_globe`](Self::is_globe), Mercator otherwise.
    #[must_use]
    pub fn projection(&self) -> crate::camera::Projection {
        if self.is_globe() {
            crate::camera::Projection::VerticalPerspective
        } else {
            crate::camera::Projection::Mercator
        }
    }

    /// The same camera, size and center altitude on Mercator: the view at a
    /// projection transition of 0. A view on Mercator is its own.
    #[must_use]
    pub(crate) fn mercator(&self) -> View {
        if !self.is_globe() {
            return self.clone();
        }
        View::new(ViewParams { projection_transition: 0.0, ..self.params })
    }

    /// The tile-to-world matrix of a tile at the view's scale.
    #[must_use]
    pub(crate) fn tile_matrix(&self, tile_id: &UnwrappedTileID) -> DMat4 {
        mercator_tile_matrix(tile_id, self.params.scale)
    }

    /// The projection of a tile, through `proj_matrix`.
    #[must_use]
    pub(crate) fn projection_data_with(&self, tile_id: &UnwrappedTileID, proj_matrix: &DMat4) -> ProjectionData {
        let mercator_matrix = *proj_matrix * self.tile_matrix(tile_id);
        self.projection().projection_data(self, tile_id, &mercator_matrix)
    }

    /// The projection of a tile, through the projection matrix.
    #[must_use]
    pub fn projection_data(&self, tile_id: &UnwrappedTileID) -> ProjectionData {
        self.projection_data_with(tile_id, &self.matrices.projection)
    }

    /// The projection of a tile whose Mercator tile-to-clip matrix is
    /// `mercator_matrix`.
    #[must_use]
    pub(crate) fn projection_data_for_matrix(
        &self,
        tile_id: &UnwrappedTileID,
        mercator_matrix: &DMat4,
    ) -> ProjectionData {
        self.projection().projection_data(self, tile_id, mercator_matrix)
    }

    /// The lowest ground near the center while terrain draws, in meters,
    /// exaggerated; `None` without terrain
    /// (`TransformState::set_terrain_min_elevation`).
    #[must_use]
    pub fn terrain_min_elevation(&self) -> Option<f64> {
        self.params.terrain_min_elevation
    }

    /// The distance from the camera to the near plane of the projection
    /// matrix, in pixels: 1, or a fiftieth of the view's height while
    /// terrain draws, as in GL JS, for the depth precision raised ground
    /// needs. The globe's matrix takes it while terrain draws too.
    #[must_use]
    pub(crate) fn near_z(&self) -> f64 {
        match self.params.terrain_min_elevation {
            Some(_) => f64::from(self.params.size.height) / 50.0,
            None => 1.0,
        }
    }

    /// The distance from the camera to the far plane of the projection
    /// matrix, in pixels at the zoom: past the farthest point of the ground
    /// that the view shows, down to the terrain's lowest ground while it
    /// draws.
    #[must_use]
    pub(crate) fn far_z(&self) -> f64 {
        match self.params.terrain_min_elevation {
            Some(min_elevation) => self.terrain_far_z(min_elevation),
            None => self.flat_far_z(),
        }
    }

    /// The far plane over a flat map, with the center at its altitude.
    fn flat_far_z(&self) -> f64 {
        let camera_to_center_distance = self.camera_to_center_distance();

        let limited_pitch = mln_style::math::clamp_f64(self.params.pitch, 0.0, max_mercator_horizon_angle());
        let camera_to_sea_level_distance =
            camera_to_center_distance + self.center_altitude_pixels().abs() / limited_pitch.cos();

        let tan_multiple = mln_style::math::clamp_f64(self.tan_fov_above_center() * limited_pitch.tan(), 0.0, 0.99);
        let horizon_multiple =
            mln_style::math::clamp_f64(self.fov_center_to_horizon().tan() * limited_pitch.tan(), 0.0, 0.99);

        // The distance to the farthest fragment that is drawn, and a little
        // more, for the precision of a fragment exactly there.
        let furthest_distance = camera_to_sea_level_distance / (1.0 - tan_multiple.min(horizon_multiple));
        furthest_distance * 1.01
    }

    /// GL JS's `cameraToSeaLevelDistance`, in pixels at the zoom: how far
    /// the camera is from sea level along the view, past the center by its
    /// altitude, at the pitch up to the horizon's limit, and at least half
    /// the distance to the center.
    #[must_use]
    pub(crate) fn camera_to_sea_level_distance(&self) -> f64 {
        let camera_to_center_distance = self.camera_to_center_distance();
        let limited_pitch = self.params.pitch.min(max_mercator_horizon_angle());
        f64::max(
            camera_to_center_distance / 2.0,
            camera_to_center_distance + self.center_altitude_pixels() / limited_pitch.cos(),
        )
    }

    /// GL JS's far plane (`calculateNearFarZ`) over ground that reaches down
    /// to `min_elevation` meters: it fits the lowest of that, the center and
    /// 100 meters below the camera. The frustum offset shortens it as it
    /// does the flat map's.
    fn terrain_far_z(&self, min_elevation: f64) -> f64 {
        let params = &self.params;
        let camera_to_center_distance = self.camera_to_center_distance();
        let pixels_per_meter = 1.0 / self.meters_per_pixel();

        // Heights in pixels at the zoom.
        let center_altitude = self.center_altitude_pixels();
        let limited_pitch = params.pitch.min(max_mercator_horizon_angle());
        let camera_to_sea_level_distance = self.camera_to_sea_level_distance();
        let camera_altitude = params.pitch.cos() * camera_to_center_distance + center_altitude;
        let lowest =
            center_altitude.min(min_elevation * pixels_per_meter).min(camera_altitude - 100.0 * pixels_per_meter);
        let lowest_plane = if lowest < 0.0 {
            camera_to_sea_level_distance - lowest / limited_pitch.cos()
        } else {
            camera_to_sea_level_distance
        };

        // How far past the center, along the lowest plane, the ray at `angle`
        // above the center of the view meets it: the law of sines in the
        // triangle of the camera, the center and that point.
        let ground_angle = PI / 2.0 + params.pitch;
        let distance_past_center = |angle: f64| {
            angle.sin() * lowest_plane / mln_style::math::clamp_f64(PI - ground_angle - angle, 0.01, PI - 0.01).sin()
        };
        let top_half_distance = distance_past_center(self.tan_fov_above_center().atan())
            .min(distance_past_center(self.fov_center_to_horizon()));

        (limited_pitch.sin() * top_half_distance + lowest_plane) * 1.01
    }

    /// The tangent of the angle from the center of the view, with the
    /// padding, to its edge farthest up the map, in units where 1 is a pixel
    /// at the center of the map: to the top edge of an upright view, and to
    /// its farthest corner as the view turns.
    fn tan_fov_above_center(&self) -> f64 {
        let params = &self.params;
        let half_width = 0.5 * f64::from(params.size.width);
        let half_height = 0.5 * f64::from(params.size.height);
        let offset = self.center_offset();
        // The view's reach from its padded center to each edge, in pixels.
        let (left, right) = (half_width + offset.x, half_width - offset.x);
        let (top, bottom) = (half_height + offset.y - params.frustum_offset.top, half_height - offset.y);
        let up = self.up_the_map();
        let reach = (up.x * right).max(-up.x * left) + (up.y * top).max(-up.y * bottom);
        reach / half_height * (params.fov / 2.0).tan()
    }

    /// The angle from the center of the view, with the padding, up the map
    /// to the horizon, where the map ends and the sky starts (GL JS's
    /// `calculateNearFarZ`): at high pitch it is below the top of the view.
    /// The sky draws the horizon [`mercator_horizon`](Self::mercator_horizon)
    /// pixels up the map from the center of the view without the padding,
    /// so the padding's offset up the map moves it, but never past 89.25
    /// degrees from straight down, nor below the center.
    fn fov_center_to_horizon(&self) -> f64 {
        let camera_to_center = self.camera_to_center_distance();
        let horizon = self.mercator_horizon();
        let min_horizon_angle = PI / 2.0 - max_mercator_horizon_angle();
        if (horizon / camera_to_center).atan() <= min_horizon_angle {
            return min_horizon_angle;
        }
        let offset = self.center_offset();
        let offset_up = DVec2::new(-offset.x, offset.y).dot(self.up_the_map());
        let farthest = camera_to_center * (max_mercator_horizon_angle() - self.params.pitch).tan();
        (mln_style::math::clamp_f64(horizon + offset_up, 0.0, farthest) / camera_to_center).atan()
    }

    /// The unit direction up the map in the view, x right and y up, which
    /// the roll, the viewport's flip and the north orientation turn: a roll
    /// of 90 degrees turns the left edge up.
    fn up_the_map(&self) -> DVec2 {
        let (sin, cos) = self.params.roll.sin_cos();
        let flip = if self.params.viewport_mode == ViewportMode::FlippedY { -1.0 } else { 1.0 };
        DVec2::from_angle(-self.north_orientation_angle()).rotate(DVec2::new(-sin, flip * cos))
    }

    /// The projection matrix with its near plane at `near_z` pixels, aligned
    /// to the pixel grid if `aligned`; zero while the size is empty.
    #[must_use]
    pub(crate) fn projection_matrix_with(&self, near_z: f64, aligned: bool) -> DMat4 {
        let params = &self.params;
        if params.size.is_empty() {
            return DMat4::ZERO;
        }

        let offset = self.center_offset();
        let width = f64::from(params.size.width);
        let height = f64::from(params.size.height);

        let camera = self.camera_pose();
        let pixels_per_meter = 1.0 / self.meters_per_pixel();
        let world_to_camera =
            camera.world_to_camera(params.scale, pixels_per_meter, params.viewport_mode == ViewportMode::FlippedY);
        let mut camera_to_clip = perspective(self.perspective_fov(), width / height, near_z, self.far_z());

        // The vanishing point moves to the center of the padded view, from -1
        // to 1 across it. An axonometric view has none.
        if !params.axonometric {
            camera_to_clip.z_axis.x = -offset.x * 2.0 / width;
            camera_to_clip.z_axis.y = offset.y * 2.0 / height;
        }

        if params.north_orientation != NorthOrientation::Upwards {
            camera_to_clip *= DMat4::from_rotation_z(-self.north_orientation_angle());
        }

        let mut proj_matrix = camera_to_clip * world_to_camera;

        if params.axonometric {
            // Without perspective, z skews x and y instead.
            proj_matrix.z_axis.w = 0.0;
            proj_matrix.z_axis.x = params.x_skew * pixels_per_meter;
            proj_matrix.z_axis.y = params.y_skew * pixels_per_meter;
        }

        // The pixel-aligned matrix, for raster tiles: the center moves to a
        // whole pixel, and by half a pixel more along a side of odd length,
        // with the shift turned by the bearing and kept within half a pixel,
        // so that rasters at right angles draw crisp.
        if aligned {
            let world_size = Projection::world_size(params.scale);
            let dx = params.x - 0.5 * world_size;
            let dy = params.y - 0.5 * world_size;

            let x_shift = f64::from(params.size.width % 2) / 2.0;
            let y_shift = f64::from(params.size.height % 2) / 2.0;
            let angle_cos = params.angle.cos();
            let angle_sin = params.angle.sin();
            let dxa = -modf_fraction(dx) + angle_cos * x_shift + angle_sin * y_shift;
            let dya = -modf_fraction(dy) + angle_cos * y_shift + angle_sin * x_shift;
            proj_matrix *= DMat4::from_translation(DVec3::new(
                if dxa > 0.5 { dxa - 1.0 } else { dxa },
                if dya > 0.5 { dya - 1.0 } else { dya },
                0.0,
            ));
        }
        proj_matrix
    }

    /// The projection matrix: from world pixels at the scale, z in meters, to
    /// clip space.
    #[cfg(any(test, feature = "bench"))]
    #[must_use]
    pub(crate) fn projection_matrix(&self) -> DMat4 {
        self.matrices.projection
    }

    #[must_use]
    pub(crate) fn inverse_projection_matrix(&self) -> DMat4 {
        self.matrices.inv_projection
    }

    /// The globe's view projection, from unit-sphere space to clip space,
    /// while [`is_globe`](Self::is_globe).
    #[must_use]
    pub(crate) fn globe_view_projection_matrix(&self) -> DMat4 {
        self.matrices.globe_view_projection
    }

    #[must_use]
    pub(crate) fn inverse_globe_view_projection_matrix(&self) -> DMat4 {
        self.matrices.inv_globe_view_projection
    }

    /// The plane, as `ax + by + cz + d`, that separates the globe's visible
    /// hemisphere from its back.
    #[must_use]
    pub(crate) fn globe_clipping_plane(&self) -> DVec4 {
        self.matrices.globe_clipping_plane
    }

    /// The camera in unit-sphere space.
    #[must_use]
    pub fn globe_camera_position(&self) -> DVec3 {
        self.matrices.globe_camera_position
    }

    /// The center of the map in the units that `coord` takes: world pixels
    /// over the tile size, at the scale.
    fn center_coordinate(&self) -> DVec3 {
        let world_size = Projection::world_size(self.params.scale);
        DVec3::new(
            (0.5 * world_size - self.params.x) / TILE_SIZE_D,
            (0.5 * world_size - self.params.y) / TILE_SIZE_D,
            0.0,
        )
    }

    /// The matrix from world space in tiles to screen pixels, y up.
    fn coordinate_point_matrix(&self, proj_matrix: &DMat4) -> DMat4 {
        let size = self.params.size;
        let pixel_matrix =
            DMat4::from_scale(DVec3::new(f64::from(size.width) / 2.0, -f64::from(size.height) / 2.0, 1.0))
                * DMat4::from_translation(DVec3::new(1.0, -1.0, 0.0));
        pixel_matrix * *proj_matrix * DMat4::from_scale(DVec3::new(TILE_SIZE_D, TILE_SIZE_D, 1.0))
    }

    // MARK: - Conversions

    /// The screen point of a location, measured from the bottom edge (y up).
    #[must_use]
    pub fn lat_lng_to_screen_coordinate(&self, lat_lng: &LatLng) -> ScreenCoordinate {
        self.lat_lng_to_screen_coordinate_at(lat_lng, 0.0)
    }

    /// The screen point of a location `height` meters above sea level,
    /// measured from the bottom edge (y up). The globe raises it along its
    /// vertical.
    #[must_use]
    pub(crate) fn lat_lng_to_screen_coordinate_at(&self, lat_lng: &LatLng, height: f64) -> ScreenCoordinate {
        self.clip_point_at(lat_lng, height).0
    }

    /// The screen point of a location and its clip-space position, whose w
    /// is negative behind the camera. Both are zero while the size is empty.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn lat_lng_to_screen_coordinate_with_clip(&self, lat_lng: &LatLng) -> (ScreenCoordinate, DVec4) {
        self.clip_point_at(lat_lng, 0.0)
    }

    /// [`lat_lng_to_screen_coordinate_with_clip`] of a location `height`
    /// meters above sea level.
    ///
    /// [`lat_lng_to_screen_coordinate_with_clip`]: Self::lat_lng_to_screen_coordinate_with_clip
    fn clip_point_at(&self, lat_lng: &LatLng, height: f64) -> (ScreenCoordinate, DVec4) {
        if self.params.size.is_empty() {
            return (ScreenCoordinate::new(0.0, 0.0), DVec4::ZERO);
        }

        if self.is_globe() {
            return VerticalPerspectiveProjection::lat_lng_to_screen_coordinate(self, lat_lng, height);
        }

        let pt = Projection::project(lat_lng, self.params.scale);
        let p = self.matrices.coord * DVec4::new(pt.x / TILE_SIZE_D, pt.y / TILE_SIZE_D, height, 1.0);
        (ScreenCoordinate::new(p.x / p.w, f64::from(self.params.size.height) - p.y / p.w), p)
    }

    /// Whether the globe hides a location from the camera; a Mercator view
    /// hides nothing.
    #[must_use]
    pub(crate) fn is_location_occluded(&self, lat_lng: &LatLng) -> bool {
        if self.params.size.is_empty() || !self.is_globe() {
            return false;
        }
        let surface = VerticalPerspectiveProjection::surface_vector(lat_lng);
        self.globe_clipping_plane().dot(surface.extend(1.0)) < 0.0
    }

    /// The tile coordinate under a screen point (y up) at `at_zoom`, as GL
    /// JS's `pointCoordinate`.
    #[must_use]
    pub(crate) fn screen_coordinate_to_tile_coordinate(&self, point: &ScreenCoordinate, at_zoom: u8) -> TileCoordinate {
        if self.params.size.is_empty() {
            return TileCoordinate { p: Point::new(0.0, 0.0), z: 0.0 };
        }

        if self.is_globe() {
            return match VerticalPerspectiveProjection::screen_coordinate_to_lat_lng(
                self,
                point,
                0.0,
                WrapMode::Unwrapped,
            ) {
                Some(lat_lng) => TileCoordinate::from_lat_lng(f64::from(at_zoom), &lat_lng),
                None => TileCoordinate { p: Point::new(0.0, 0.0), z: 0.0 },
            };
        }

        self.mercator_screen_coordinate_to_tile_coordinate(point, at_zoom, 0.0)
    }

    /// The tile coordinate at `at_zoom` where the line of sight through
    /// `point` (y up) meets the plane `height` meters above sea level; where
    /// the line doesn't descend, the point at clip depth 0.
    fn mercator_screen_coordinate_to_tile_coordinate(
        &self,
        point: &ScreenCoordinate,
        at_zoom: u8,
        height: f64,
    ) -> TileCoordinate {
        let flipped_y = f64::from(self.params.size.height) - point.y;

        // The point's depth isn't known: unproject it at two depths, and take
        // where the line between them meets the plane.
        let inverted = self.matrices.inverted_from_center;
        let offset0 = inverted.project_point3(DVec3::new(point.x, flipped_y, 0.0));
        let offset1 = inverted.project_point3(DVec3::new(point.x, flipped_y, 1.0));

        let z0 = offset0.z;
        let z1 = offset1.z;
        // `z0 - 0.0` is `z0`, so sea level is the flat map's plane to the bit.
        let t = if z0 <= z1 { 0.0 } else { -(z0 - height) / (z1 - z0) };

        let offset = offset0 + (offset1 - offset0) * t;
        let center = self.center_coordinate();
        let factor = f64::from(1i32.wrapping_shl(u32::from(at_zoom)));
        let scale = self.params.scale;
        TileCoordinate {
            p: Point::new((center.x + offset.x) / scale * factor, (center.y + offset.y) / scale * factor),
            z: f64::from(at_zoom),
        }
    }

    /// The location under `point`, measured from the bottom edge (y up).
    /// Fails with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for a
    /// NaN or infinite point.
    pub(crate) fn screen_coordinate_to_lat_lng(
        &self,
        point: &ScreenCoordinate,
        wrap_mode: WrapMode,
    ) -> Result<LatLng, Error> {
        self.screen_coordinate_to_lat_lng_at(point, 0.0, wrap_mode)
    }

    /// The location where the line of sight through `point`, measured from
    /// the bottom edge (y up), meets the plane `height` meters above sea
    /// level; where the line doesn't descend, as at sea level, the point it
    /// passes a little way from the camera. The globe meets the sphere
    /// `height` meters above sea level, and a line that misses it the
    /// nearest point of that sphere's horizon. Fails as
    /// `screen_coordinate_to_lat_lng`.
    pub fn screen_coordinate_to_lat_lng_at(
        &self,
        point: &ScreenCoordinate,
        height: f64,
        wrap_mode: WrapMode,
    ) -> Result<LatLng, Error> {
        // Without a size the point is Mercator's, as the tile coordinate is.
        if self.is_globe() && !self.params.size.is_empty() {
            if point.x.is_nan() || point.y.is_nan() {
                return Err(Error::invalid_argument("screen coordinate must not be NaN"));
            }
            if !point.x.is_finite() || !point.y.is_finite() {
                return Err(Error::invalid_argument("screen coordinate must not be infinite"));
            }
            return VerticalPerspectiveProjection::screen_coordinate_to_lat_lng(self, point, height, wrap_mode)
                .ok_or_else(|| Error::invalid_argument("the camera shows no location"));
        }
        let coord = if self.params.size.is_empty() {
            self.screen_coordinate_to_tile_coordinate(point, 0)
        } else {
            self.mercator_screen_coordinate_to_tile_coordinate(point, 0, height)
        };
        Projection::unproject(&coord.p, 1.0 / TILE_SIZE_D, wrap_mode)
    }

    /// The line of sight through `point`, measured from the bottom edge (y
    /// up), from the near plane to the far plane: each end's Web Mercator
    /// point at zoom 0 (x in the world copy the view shows it in) and its
    /// height in meters. `None` on the globe, while the view shows nothing,
    /// and for a point that isn't finite.
    #[must_use]
    pub fn line_of_sight(&self, point: &ScreenCoordinate) -> Option<(DVec3, DVec3)> {
        if self.is_globe() || self.matrices.projection == DMat4::ZERO {
            return None;
        }
        let flipped_y = f64::from(self.params.size.height) - point.y;
        // The matrices are OpenGL's: clip depth runs from -1 at the near plane
        // to 1 at the far plane.
        let inverted = self.matrices.inverted_from_center;
        let center = self.center_coordinate();
        let scale = self.params.scale;
        let world = |depth: f64| {
            let offset = inverted.project_point3(DVec3::new(point.x, flipped_y, depth));
            DVec3::new((center.x + offset.x) / scale, (center.y + offset.y) / scale, offset.z)
        };
        let (near, far) = (world(-1.0), world(1.0));
        (near.is_finite() && far.is_finite()).then_some((near, far))
    }

    /// How much farther the bottom-left corner of the view is from the
    /// camera than the center, by which queries pad what they look for; 0
    /// while the size is empty.
    #[must_use]
    pub(crate) fn max_pitch_scale_factor(&self) -> f64 {
        if self.params.size.is_empty() {
            return 0.0;
        }
        // The globe pads queries with the Mercator factor, as GL JS does, from
        // the corner's unclamped Mercator position.
        let corner = self.mercator_screen_coordinate_to_tile_coordinate(
            &ScreenCoordinate::new(0.0, f64::from(self.params.size.height)),
            0,
            0.0,
        );
        // A point on the screen has a LatLng unless the view holds NaN.
        let Ok(lat_lng) = Projection::unproject(&corner.p, 1.0 / TILE_SIZE_D, WrapMode::Unwrapped) else {
            return 0.0;
        };

        let scale = self.params.scale;
        let pt = if self.is_globe() {
            Point::new(corner.p.x * scale, corner.p.y * scale)
        } else {
            let pt = Projection::project(&lat_lng, scale);
            Point::new(pt.x / TILE_SIZE_D, pt.y / TILE_SIZE_D)
        };
        let top_point = self.matrices.coord * DVec4::new(pt.x, pt.y, 0.0, 1.0);
        top_point.w / self.camera_to_center_distance()
    }
}
