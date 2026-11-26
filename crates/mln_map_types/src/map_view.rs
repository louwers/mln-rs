//! One view of a map: the camera, size and projection that a map snapshot or a
//! rendered frame shows.

use mln_style::error::Error;
use mln_style::geo::{LatLng, WrapMode};
use mln_style::size::Size;
use mln_style::types::ProjectionDefinition;
use mln_util::constants;
use mln_util::geo::{EdgeInsets, NorthOrientation};

use crate::camera::CameraOptions;
use crate::camera_limits::{CameraLimits, MAX_FIELD_OF_VIEW, MIN_FIELD_OF_VIEW};
use crate::mode::{ConstrainMode, ViewportMode};
use crate::projection_mode::{AxonometricMode, ProjectionMode};
use crate::transform_state::TransformState;
use crate::view::{View, ViewParams, map_offset, zoom_scale};

/// A camera with every member set: the camera that a map has, where
/// [`CameraOptions`] is a change to one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Camera {
    /// The coordinate at the center. It keeps the copy of the world that the
    /// camera is in, so its longitude can be outside -180 to 180.
    pub center: LatLng,
    /// The altitude of the center, in meters above sea level.
    pub center_altitude: f64,
    /// Insets from the map's edges. The camera centers within the rest.
    pub padding: EdgeInsets,
    pub zoom: f64,
    /// The rotation from north, in degrees.
    pub bearing: f64,
    /// The tilt toward the horizon, in degrees. 0 looks straight down.
    pub pitch: f64,
    /// The roll, in degrees.
    pub roll: f64,
    /// The vertical field of view, in degrees.
    pub fov: f64,
}

impl From<Camera> for CameraOptions {
    /// Options that set every member of the camera, and no anchor.
    fn from(camera: Camera) -> Self {
        CameraOptions {
            center: Some(camera.center),
            center_altitude: Some(camera.center_altitude),
            padding: Some(camera.padding),
            anchor: None,
            zoom: Some(camera.zoom),
            bearing: Some(camera.bearing),
            pitch: Some(camera.pitch),
            roll: Some(camera.roll),
            fov: Some(camera.fov),
        }
    }
}

/// A size in logical pixels, and the physical pixels in each logical pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogicalExtent {
    pub size: Size,
    pub scale_factor: f64,
}

impl LogicalExtent {
    #[must_use]
    pub fn new(size: Size, scale_factor: f64) -> LogicalExtent {
        LogicalExtent { size, scale_factor }
    }
}

/// How a map orients and constrains its view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub north_orientation: NorthOrientation,
    pub constrain_mode: ConstrainMode,
    pub viewport_mode: ViewportMode,
    /// Insets that move the vanishing point off the center of the map.
    pub frustum_offset: EdgeInsets,
    /// Whether the center stands on the terrain, at its height under the
    /// center, once that has loaded. The ground decides the altitude, so the
    /// camera ignores the altitude it is given, such as the style's
    /// `centerAltitude`: at rest the center stands on the ground, and a
    /// command eases the altitude onto the ground where it leaves the center.
    /// When this is off, every camera keeps the altitude it is given. Over
    /// terrain, a command's eye stays above the ground either way. Without
    /// terrain it changes nothing. On by default.
    pub center_clamped_to_ground: bool,
    /// Whether drags and zooms around a point on the globe turn the bearing,
    /// so the planet keeps its orientation, and a drag can carry the camera
    /// over a pole. Off by default: they keep the bearing, as MapLibre GL JS
    /// does. It changes nothing while the map draws Mercator.
    pub globe_free_bearing: bool,
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport {
            north_orientation: NorthOrientation::default(),
            constrain_mode: ConstrainMode::default(),
            viewport_mode: ViewportMode::default(),
            frustum_offset: EdgeInsets::default(),
            center_clamped_to_ground: true,
            globe_free_bearing: false,
        }
    }
}

/// One view of a map: its camera, size, and projection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapView {
    pub camera: Camera,
    pub extent: LogicalExtent,
    /// Sets every member when it comes from a map. A member that a view leaves
    /// unset takes the default of [`View`].
    pub projection_mode: ProjectionMode,
    pub viewport: Viewport,
    /// The style's projection at the camera's zoom. A view from a map never
    /// names `globe`, which [`to_state`](Self::to_state) resolves at the
    /// camera's zoom.
    pub projection: ProjectionDefinition,
}

impl MapView {
    /// A transform state that shows exactly this view, to convert coordinates
    /// for it.
    ///
    /// The state takes the camera as it is: it keeps the center's copy of the
    /// world, and has no bounds and the widest zoom and pitch limits, so no
    /// constraint of the map that the view came from moves the camera.
    ///
    /// `Err` for a view that no transform can show: a camera member that isn't
    /// finite, a zoom, pitch or field of view outside the limits of every map,
    /// a padding or frustum offset below zero, an empty size, a scale factor
    /// that isn't positive, or a skew that isn't finite.
    pub fn to_state(&self) -> Result<TransformState, Error> {
        self.validate()?;
        let camera = &self.camera;
        let viewport = &self.viewport;
        let projection = self.projection.at_zoom(camera.zoom);
        let projection_transition = projection.transition_state();
        let scale = zoom_scale(camera.zoom);
        let (x, y) = map_offset(&camera.center, scale, projection_transition > 0.0);
        let params = ViewParams {
            size: self.extent.size,
            frustum_offset: viewport.frustum_offset,
            north_orientation: viewport.north_orientation,
            viewport_mode: viewport.viewport_mode,
            constrain_mode: viewport.constrain_mode,
            center_clamped_to_ground: viewport.center_clamped_to_ground,
            globe_free_bearing: viewport.globe_free_bearing,
            projection_definition: projection,
            projection_transition,
            padding: camera.padding,
            x,
            y,
            center_altitude: camera.center_altitude,
            scale,
            angle: -mln_style::math::deg2rad(camera.bearing),
            pitch: mln_style::math::deg2rad(camera.pitch),
            roll: mln_style::math::deg2rad(camera.roll),
            fov: mln_style::math::deg2rad(camera.fov),
            x_skew: self.projection_mode.x_skew.unwrap_or(0.0),
            y_skew: self.projection_mode.y_skew.unwrap_or(1.0),
            axonometric: self.projection_mode.axonometric.is_some_and(AxonometricMode::as_bool),
            center_wrap: WrapMode::Unwrapped,
            terrain_min_elevation: None,
        };
        Ok(TransformState::from_parts(params, CameraLimits::widest()))
    }

    /// The view that this describes, to draw it or convert coordinates for
    /// it. `Err`: as [`to_state`](Self::to_state).
    pub fn to_view(&self) -> Result<View, Error> {
        Ok(self.to_state()?.view().clone())
    }

    fn validate(&self) -> Result<(), Error> {
        let camera = &self.camera;
        let finite = [camera.center_altitude, camera.zoom, camera.bearing, camera.pitch, camera.roll, camera.fov];
        if !finite.iter().all(|value| value.is_finite()) {
            return Err(Error::invalid_argument("view camera members must be finite"));
        }
        let projection = &self.projection;
        if !projection.transition.is_finite() {
            return Err(Error::invalid_argument("view projection transition must be finite"));
        }
        let limits = TransformState::default();
        // The globe lets the zoom fall below the minimum toward the poles.
        let globe = projection.at_zoom(camera.zoom).transition_state() > 0.0;
        let min_zoom = limits.min_zoom_at_latitude_with(camera.center.latitude(), globe);
        if !(min_zoom..=constants::MAX_ZOOM).contains(&camera.zoom)
            || !(mln_style::math::rad2deg(constants::PITCH_MIN)..=mln_style::math::rad2deg(constants::PITCH_MAX))
                .contains(&camera.pitch)
            || !(mln_style::math::rad2deg(MIN_FIELD_OF_VIEW)..=mln_style::math::rad2deg(MAX_FIELD_OF_VIEW))
                .contains(&camera.fov)
        {
            return Err(Error::invalid_argument(
                "view camera zoom, pitch and field of view must be within the limits of a map",
            ));
        }
        if !valid_insets(&camera.padding) || !valid_insets(&self.viewport.frustum_offset) {
            return Err(Error::invalid_argument("view padding and frustum offset must be finite and at least 0"));
        }
        let extent = &self.extent;
        if extent.size.is_empty() || !extent.scale_factor.is_finite() || extent.scale_factor <= 0.0 {
            return Err(Error::invalid_argument("view extent dimensions and scale factor must be positive"));
        }
        let mode = &self.projection_mode;
        if !mode.x_skew.is_none_or(f64::is_finite) || !mode.y_skew.is_none_or(f64::is_finite) {
            return Err(Error::invalid_argument("view projection skews must be finite"));
        }
        Ok(())
    }
}

fn valid_insets(insets: &EdgeInsets) -> bool {
    [insets.top, insets.left, insets.bottom, insets.right].iter().all(|value| value.is_finite() && *value >= 0.0)
}
