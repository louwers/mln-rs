use std::f64::consts::PI;

use geometry::Point;
use glam::{DQuat, DVec3};
use mln_style_types::ProjectionDefinition;
use mln_util::constants;
use mln_util::constants::{DEFAULT_MAX_ZOOM, DEFAULT_PITCH_MAX, M2PI, PITCH_MAX, TILE_SIZE_D};
use mln_util::error::Error;
use mln_util::geo::{EdgeInsets, LatLng, LatLngBounds, ScreenCoordinate, WrapMode};
use mln_util::logging::{Event, Log};
use mln_util::projection::Projection;
use mln_util::size::Size;

use crate::camera::{CameraOptions, FreeCameraOptions};
use crate::camera_pose::CameraPose;
use crate::mode::{ConstrainMode, ViewportMode};
use crate::vertical_perspective_projection::VerticalPerspectiveProjection;
use crate::view::{NorthOrientation, View, ViewParams, max_mercator_horizon_angle, scale_zoom, zoom_scale};

fn lat_lng_from_mercator(mercator_coordinate: Point<f64>, wrap_mode: WrapMode) -> Result<LatLng, Error> {
    LatLng::with_mode(
        mln_math::rad2deg(2.0 * (PI - mercator_coordinate.y * 2.0 * PI).exp().atan() - (PI / 2.0)),
        mercator_coordinate.x * 360.0 - 180.0,
        wrap_mode,
    )
}

/// A map's camera with the limits that constrain it: the [`View`] it shows
/// now, and the bounds, zoom, pitch and field of view it may move within.
///
/// Setters ignore numbers that aren't finite.
#[derive(Clone, Debug, PartialEq)]
pub struct TransformState {
    view: View,

    bounds: LatLngBounds,
    min_scale: f64,
    max_scale: f64,
    min_pitch: f64,
    max_pitch: f64,
    min_fov: f64,
    max_fov: f64,
}

impl Default for TransformState {
    fn default() -> Self {
        TransformState::new(ConstrainMode::HeightOnly, ViewportMode::Default)
    }
}

impl TransformState {
    #[must_use]
    pub fn new(constrain_mode: ConstrainMode, viewport_mode: ViewportMode) -> Self {
        TransformState {
            view: View::new(ViewParams::new(constrain_mode, viewport_mode)),
            bounds: LatLngBounds::default(),
            min_scale: 1.0,
            max_scale: 2.0f64.powf(f64::from(DEFAULT_MAX_ZOOM)),
            min_pitch: constants::PITCH_MIN,
            max_pitch: DEFAULT_PITCH_MAX,
            min_fov: mln_math::deg2rad(0.1),
            max_fov: mln_math::deg2rad(150.0),
        }
    }

    /// The view the camera shows now.
    #[must_use]
    pub fn view(&self) -> &View {
        &self.view
    }

    /// Changes the view's parameters with `change`, and remakes the view if
    /// they changed.
    fn update(&mut self, change: impl FnOnce(&mut ViewParams)) {
        let mut params = *self.view.params();
        change(&mut params);
        params.center_wrap = if params.projection_transition > 0.0 && self.bounds == LatLngBounds::default() {
            WrapMode::Wrapped
        } else {
            WrapMode::Unwrapped
        };
        if params != *self.view.params() {
            self.view = View::new(params);
        }
    }

    // MARK: - Projection

    pub fn set_projection_definition(&mut self, definition: &ProjectionDefinition) {
        let transition = definition.transition_state();
        let previous = self.view.projection_transition();
        self.update(|p| {
            p.projection_definition = *definition;
            p.projection_transition = transition;
        });
        // As GL JS's `migrateProjection` does: the globe allows lower zooms,
        // latitudes past Mercator's and a center on another world copy, so
        // Mercator's limits and its wrapped center apply again.
        let leaves_globe = transition == 0.0 && previous > 0.0;
        if leaves_globe && !self.view.size().is_empty() {
            let wrap_mode =
                if self.bounds == LatLngBounds::default() { WrapMode::Wrapped } else { WrapMode::Unwrapped };
            let center = self.view.center(wrap_mode);
            let zoom = self.view.zoom();
            self.set_lat_lng_zoom(&center, zoom);
        }
    }

    // MARK: - Free camera

    fn update_state_from_camera(&mut self, camera: &CameraPose) {
        let position = camera.position;
        let DVec3 { x: dx, y: dy, z: dz } = camera.forward();

        let (new_roll, mut new_pitch, new_bearing) = camera.orientation_angles();
        new_pitch = mln_math::clamp_f64(new_pitch, self.min_pitch, self.max_pitch);

        // The zoom from the camera's altitude, and how far along its line of
        // sight the ground is.
        let center_distance = self.view.camera_to_center_distance();
        let zoom;
        let travel;
        if dz < -1.0e-9 && position.z > 1.0e-9 && new_pitch <= max_mercator_horizon_angle() {
            zoom = (center_distance / (position.z / new_pitch.cos() * TILE_SIZE_D)).log2();
            travel = -position.z / dz;
        } else {
            zoom = 14.0;
            let new_scale = mln_math::clamp_f64(2.0f64.powf(zoom), self.min_scale, self.max_scale);
            travel = center_distance / new_scale / TILE_SIZE_D;
        }

        let mercator_point = Point::new(position.x + dx * travel, position.y + dy * travel);
        let Ok(center) = lat_lng_from_mercator(mercator_point, WrapMode::Unwrapped) else {
            return;
        };
        let zoom = f64::min(zoom, scale_zoom(self.max_scale));
        self.set_lat_lng_zoom(&center, zoom);

        let mercator_z = position.z + dz * travel;
        let alt_m = mercator_z
            * Projection::get_meters_per_pixel_at_latitude(self.view.center(WrapMode::Unwrapped).latitude(), 0.0)
            * TILE_SIZE_D;
        self.set_center_altitude(alt_m);
        self.set_bearing(new_bearing);
        self.set_pitch(new_pitch);
        self.set_roll(new_roll);
    }

    /// `position` with its altitude clamped to the zoom range, or `None` for
    /// a position that isn't a number.
    fn constrained_camera_position(&self, position: DVec3) -> Option<DVec3> {
        if position.is_nan() {
            return None;
        }
        let max_world_size = Projection::world_size(2.0f64.powf(self.get_max_zoom()));
        let min_world_size = Projection::world_size(2.0f64.powf(self.get_min_zoom()));
        let dist_to_center = self.view.camera_to_center_distance();
        Some(DVec3::new(
            position.x,
            position.y,
            mln_math::clamp_f64(position.z, dist_to_center / max_world_size, dist_to_center / min_world_size),
        ))
    }

    /// `orientation` without the twist that a map's camera can't take, or
    /// `None` for an orientation that isn't a rotation or that looks along
    /// its up direction.
    fn constrained_camera_orientation(orientation: DQuat) -> Option<DQuat> {
        if orientation.is_nan() || orientation.length() == 0.0 {
            return None;
        }
        let unit_quat = orientation.normalize();
        CameraPose::orientation_from_frame(unit_quat * DVec3::NEG_Z, unit_quat * DVec3::NEG_Y)
    }

    pub fn set_free_camera_options(&mut self, options: &FreeCameraOptions) {
        if !self.valid() {
            return;
        }

        let mut camera = self.view.camera_pose();
        let mut changed = false;
        if let Some(orientation) = options.orientation
            && orientation != camera.orientation
            && let Some(orientation) = Self::constrained_camera_orientation(orientation)
        {
            camera.orientation = orientation;
            changed = true;
        }

        if let Some(position) = options.position
            && position != camera.position
            && let Some(position) = self.constrained_camera_position(position)
        {
            camera.position = position;
            changed = true;
        }

        if changed {
            self.update_state_from_camera(&camera);
        }
    }

    // MARK: - Dimensions

    pub fn set_size(&mut self, size: Size) {
        self.update(|p| p.size = size);
    }

    pub fn set_frustum_offset(&mut self, frustum_offset: &EdgeInsets) {
        self.update(|p| p.frustum_offset = *frustum_offset);
    }

    pub fn set_north_orientation(&mut self, orientation: NorthOrientation) {
        self.update(|p| p.north_orientation = orientation);
    }

    #[must_use]
    pub fn get_constrain_mode(&self) -> ConstrainMode {
        self.view.params().constrain_mode
    }

    pub fn set_constrain_mode(&mut self, mode: ConstrainMode) {
        self.update(|p| p.constrain_mode = mode);
    }

    pub fn set_viewport_mode(&mut self, mode: ViewportMode) {
        self.update(|p| p.viewport_mode = mode);
    }

    pub fn set_edge_insets(&mut self, padding: &EdgeInsets) {
        self.update(|p| p.padding = *padding);
    }

    // MARK: - Camera options

    /// The camera, with `padding` in place of its own if given.
    #[must_use]
    pub fn get_camera_options(&self, padding: Option<EdgeInsets>) -> CameraOptions {
        let mut camera = self.view.camera();
        if let Some(padding) = padding {
            camera.padding = padding;
        }
        CameraOptions::from(camera)
    }

    // MARK: - Zoom

    /// Lowest zoom allowed with the map centered at this latitude: the globe
    /// lets the zoom fall below `get_min_zoom()` toward the poles by the
    /// anchored-zoom adjustment, so the planet keeps its size on screen.
    #[must_use]
    pub fn get_min_zoom_at_latitude(&self, latitude: f64) -> f64 {
        self.get_min_zoom_at_latitude_with(latitude, self.view.is_globe())
    }

    /// [`get_min_zoom_at_latitude`](Self::get_min_zoom_at_latitude) for a
    /// camera on the globe or not, whatever the current projection.
    #[must_use]
    pub fn get_min_zoom_at_latitude_with(&self, latitude: f64, globe: bool) -> f64 {
        if !globe {
            return self.get_min_zoom();
        }
        // The globe's constraint never raises the scale, whichever projection
        // the map is in now.
        scale_zoom(self.min_scale) + VerticalPerspectiveProjection::zoom_adjustment(0.0, latitude)
    }

    /// The center `constrain` would settle on for this one at this zoom: on
    /// the globe the zoom decides how close to a pole the center can be.
    #[must_use]
    pub fn constrained_center(&self, lat_lng: &LatLng, zoom: f64) -> LatLng {
        if self.get_constrain_mode() == ConstrainMode::None || !self.view.is_globe() {
            return *lat_lng;
        }
        let max_latitude = VerticalPerspectiveProjection::max_latitude(
            &self.view,
            mln_math::clamp_f64(zoom, self.get_min_zoom_at_latitude(lat_lng.latitude()), self.get_max_zoom()),
        );
        LatLng::saturating(
            mln_math::clamp_f64(lat_lng.latitude(), -max_latitude, max_latitude),
            lat_lng.longitude(),
            WrapMode::Unwrapped,
        )
    }

    // MARK: - Bounds

    pub fn set_lat_lng_bounds(&mut self, bounds: LatLngBounds) {
        if bounds != self.bounds {
            self.bounds = bounds;
            let lat_lng = self.view.center(WrapMode::Unwrapped);
            let zoom = self.view.zoom();
            self.set_lat_lng_zoom(&lat_lng, zoom);
            self.update(|_| ());
        }
    }

    #[must_use]
    pub fn get_lat_lng_bounds(&self) -> LatLngBounds {
        self.bounds
    }

    pub fn set_min_zoom(&mut self, min_zoom: f64) {
        if min_zoom <= self.get_max_zoom() {
            self.min_scale = zoom_scale(mln_math::clamp_f64(min_zoom, constants::MIN_ZOOM, constants::MAX_ZOOM));
        }
    }

    #[must_use]
    pub fn get_min_zoom(&self) -> f64 {
        scale_zoom(self.constrained_min_scale())
    }

    fn constrained_min_scale(&self) -> f64 {
        self.constrain(self.min_scale, self.view.x(), self.view.y()).0
    }

    pub fn set_max_zoom(&mut self, max_zoom: f64) {
        if max_zoom >= self.get_min_zoom() {
            self.max_scale = zoom_scale(mln_math::clamp_f64(max_zoom, constants::MIN_ZOOM, constants::MAX_ZOOM));
        }
    }

    #[must_use]
    pub fn get_max_zoom(&self) -> f64 {
        scale_zoom(self.max_scale)
    }

    pub fn set_min_pitch(&mut self, pitch: f64) {
        if pitch <= self.max_pitch {
            self.min_pitch = mln_math::clamp_f64(pitch, constants::PITCH_MIN, self.max_pitch);
        } else {
            Log::warning(Event::General, "Trying to set minimum pitch to larger than maximum pitch, no changes made.");
        }
    }

    #[must_use]
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

    #[must_use]
    pub fn get_max_pitch(&self) -> f64 {
        self.max_pitch
    }

    #[must_use]
    pub fn get_min_field_of_view(&self) -> f64 {
        self.min_fov
    }

    #[must_use]
    pub fn get_max_field_of_view(&self) -> f64 {
        self.max_fov
    }

    // MARK: - Rotation

    /// Sets the map's rotation, in radians counterclockwise: the negated
    /// bearing.
    pub fn set_bearing(&mut self, angle: f64) {
        if angle.is_finite() {
            self.update(|p| p.angle = angle);
        }
    }

    pub fn set_field_of_view(&mut self, fov: f64) {
        if fov.is_finite() {
            self.update(|p| p.fov = fov);
        }
    }

    pub fn set_roll(&mut self, roll: f64) {
        if roll.is_finite() {
            self.update(|p| p.roll = roll);
        }
    }

    pub fn set_pitch(&mut self, pitch: f64) {
        if pitch.is_finite() {
            self.update(|p| p.pitch = pitch);
        }
    }

    pub fn set_x_skew(&mut self, skew: f64) {
        if skew.is_finite() {
            self.update(|p| p.x_skew = skew);
        }
    }

    pub fn set_y_skew(&mut self, skew: f64) {
        if skew.is_finite() {
            self.update(|p| p.y_skew = skew);
        }
    }

    pub fn set_axonometric(&mut self, axonometric: bool) {
        self.update(|p| p.axonometric = axonometric);
    }

    // MARK: - Validity

    /// Whether the view shows anything and its scale is within the zoom
    /// range. Toward the poles the globe goes below the minimum scale;
    /// `set_lat_lng_zoom` holds it to the floor there.
    #[must_use]
    pub fn valid(&self) -> bool {
        let scale = self.view.scale();
        self.view.is_valid()
            && (if self.view.is_globe() { scale > 0.0 } else { scale >= self.min_scale })
            && scale <= self.max_scale
    }

    // MARK: - Constraints

    fn rotated_north(&self) -> bool {
        matches!(self.view.north_orientation(), NorthOrientation::Leftwards | NorthOrientation::Rightwards)
    }

    /// In [`ConstrainMode::Screen`], the scale and center that keep the
    /// lat/lng bounds on the screen at `scale`: the center only if it had to
    /// move. Other modes return `scale` unchanged and no center.
    #[must_use]
    pub fn constrain_screen(&self, scale: f64) -> (f64, Option<LatLng>) {
        if self.get_constrain_mode() != ConstrainMode::Screen {
            return (scale, None);
        }
        let mut zoom = scale_zoom(scale);
        let mut options = CameraOptions::default();
        self.constrain_camera_and_zoom_to_bounds(&mut options, &mut zoom);
        (zoom_scale(zoom), options.center)
    }

    /// The `(scale, x, y)` nearest the given ones that the constrain mode
    /// allows.
    #[must_use]
    pub fn constrain(&self, mut scale: f64, mut x: f64, mut y: f64) -> (f64, f64, f64) {
        let constrain_mode = self.get_constrain_mode();
        if constrain_mode == ConstrainMode::None || constrain_mode == ConstrainMode::Screen {
            return (scale, x, y);
        }

        // The globe has no off-world area: the center goes past the Mercator
        // range toward a pole as far as the zoom allows.
        if self.view.is_globe() {
            let max_latitude = VerticalPerspectiveProjection::max_latitude(&self.view, scale_zoom(scale));
            let max_y = Projection::world_size(scale) / M2PI * mln_math::deg2rad(max_latitude).tan().asinh();
            y = mln_math::clamp_f64(y, -max_y, max_y);
            return (scale, x, y);
        }

        let size = self.view.size();
        let width = f64::from(size.width);
        let height = f64::from(size.height);

        // Constrain scale to avoid zooming out far enough to show off-world
        // areas on the Y axis.
        let ratio_y = (if self.rotated_north() { width } else { height }) / TILE_SIZE_D;
        scale = mln_math::fmax(scale, ratio_y);

        // Constrain min/max pan to avoid showing off-world areas on the Y axis.
        let max_y = (scale * TILE_SIZE_D - (if self.rotated_north() { width } else { height })) / 2.0;
        y = f64::max(-max_y, f64::min(y, max_y));

        if constrain_mode == ConstrainMode::WidthAndHeight {
            // Constrain min/max pan to avoid showing off-world areas on the X axis.
            let max_x = (scale * TILE_SIZE_D - (if self.rotated_north() { height } else { width })) / 2.0;
            x = f64::max(-max_x, f64::min(x, max_x));
        }
        (scale, x, y)
    }

    /// Moves the camera to the position nearest its own at `scale` that the
    /// constrain mode allows.
    pub fn constrain_position(&mut self, scale: f64) {
        let (scale, x, y) = self.constrain(scale, self.view.x(), self.view.y());
        if scale.is_finite() && x.is_finite() && y.is_finite() {
            self.update(|p| {
                p.scale = scale;
                p.x = x;
                p.y = y;
            });
        }
    }

    pub fn constrain_camera_and_zoom_to_bounds(&self, requested_camera: &mut CameraOptions, requested_zoom: &mut f64) {
        if self.get_constrain_mode() != ConstrainMode::Screen || self.get_lat_lng_bounds() == LatLngBounds::default() {
            return;
        }

        let view = &self.view;
        let current_center = view.center(WrapMode::Unwrapped);
        let center_lat_lng = requested_camera.center.unwrap_or(current_center);

        let mut anchor_offset = Point::new(0.0, 0.0);
        let requested_scale = zoom_scale(*requested_zoom);

        // The transition will move the camera so that the anchor's location
        // stays under it: the requested center is tested from where that
        // leaves it.
        if let Some(mut anchor) = requested_camera.anchor {
            anchor.y = f64::from(view.size().height) - anchor.y;
            // The location under the anchor at the requested scale.
            let mut state = self.clone();
            state.set_lat_lng_zoom(&current_center, scale_zoom(requested_scale));
            // An anchor with no location is ignored, as the transition
            // ignores it.
            if let (Ok(anchor_lat_lng), Ok(screen_lat_lng)) = (
                view.screen_coordinate_to_lat_lng(&anchor, WrapMode::Unwrapped),
                state.view.screen_coordinate_to_lat_lng(&anchor, WrapMode::Unwrapped),
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
        let current_center = Projection::project(&current_center, requested_scale);

        let min_y = ne_bounds.y;
        let max_y = sw_bounds.y;
        let min_x = sw_bounds.x;
        let max_x = ne_bounds.x;

        let start_x = center.x;
        let start_y = center.y;

        let mut result_x = start_x;
        let mut result_y = start_y;

        let screen_width = view.size().width;
        let screen_height = view.size().height;

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
            *requested_zoom += scale_zoom(max_scale);

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
    #[must_use]
    pub fn get_min_zoom_for_screen_bounds(&self) -> Option<f64> {
        if self.get_constrain_mode() != ConstrainMode::Screen || self.bounds == LatLngBounds::default() {
            return None;
        }
        // The bounds' extent on screen grows with the scale alone, so they
        // fill the screen at the same zoom wherever the center is.
        let ne = Projection::project(&self.bounds.northeast(), 1.0);
        let sw = Projection::project(&self.bounds.southwest(), 1.0);
        let size = self.view.size();
        let scale_x = f64::from(size.width) / (ne.x - sw.x);
        let scale_y = f64::from(size.height) / (sw.y - ne.y);
        Some(scale_zoom(f64::max(scale_x, scale_y)))
    }

    // MARK: - Moving the camera

    /// Recenter the map so that the given coordinate is located at the given
    /// point on screen.
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// NaN or infinite anchor.
    pub fn move_lat_lng(&mut self, lat_lng: &LatLng, anchor: &ScreenCoordinate) -> Result<(), Error> {
        let view = &self.view;
        if view.is_globe() {
            if let Some(center) = VerticalPerspectiveProjection::center_for_location_at_point(view, lat_lng, anchor) {
                let target = self.constrained_center(&center, view.zoom());
                let zoom = view.zoom()
                    + VerticalPerspectiveProjection::zoom_adjustment(
                        view.center(WrapMode::Unwrapped).latitude(),
                        target.latitude(),
                    );
                self.set_lat_lng_zoom(&target, zoom);
            }
            return Ok(());
        }
        let scale = view.scale();
        let center_coord = Projection::project(&view.center(WrapMode::Unwrapped), scale);
        let lat_lng_coord = Projection::project(lat_lng, scale);
        let anchor_coord = Projection::project(&view.screen_coordinate_to_lat_lng(anchor, WrapMode::Unwrapped)?, scale);
        let center = Projection::unproject(
            &Point::new(
                center_coord.x + lat_lng_coord.x - anchor_coord.x,
                center_coord.y + lat_lng_coord.y - anchor_coord.y,
            ),
            scale,
            WrapMode::Unwrapped,
        )?;
        // Wrapped, as easeTo and GL JS's setLocationAtPoint leave it, unless
        // bounds keep it in their range.
        let target = if self.bounds == LatLngBounds::default() { center.wrapped() } else { center };
        let zoom = view.zoom();
        self.set_lat_lng_zoom(&target, zoom);
        Ok(())
    }

    pub fn set_lat_lng_zoom(&mut self, lat_lng: &LatLng, zoom: f64) {
        if !zoom.is_finite() {
            return;
        }
        let globe = self.view.is_globe();
        let mut requested = *lat_lng;
        // Globe centers arrive wrapped, and bounds across the antimeridian
        // reach past 180 degrees: take the copy of the longitude nearest the
        // bounds before constraining it.
        if globe && self.bounds != LatLngBounds::default() {
            let turns = ((self.bounds.center().longitude() - lat_lng.longitude()) / constants::DEGREES_MAX).round();
            requested = LatLng::saturating(
                lat_lng.latitude(),
                lat_lng.longitude() + turns * constants::DEGREES_MAX,
                WrapMode::Unwrapped,
            );
        }
        let mut constrained = self.bounds.constrain(&requested);
        // The globe's tile cover picks each tile's wrap from the center, so a
        // center that jumped a whole world at the antimeridian would re-key
        // every tile; keep it on the copy nearest the current one.
        if globe && self.bounds == LatLngBounds::default() {
            let current = self.view.center(WrapMode::Unwrapped).longitude();
            if current.is_finite() {
                let turns = ((current - constrained.longitude()) / constants::DEGREES_MAX).round();
                constrained = LatLng::saturating(
                    constrained.latitude(),
                    constrained.longitude() + turns * constants::DEGREES_MAX,
                    WrapMode::Unwrapped,
                );
            }
        }

        let min_scale = if globe {
            zoom_scale(self.get_min_zoom_at_latitude(constrained.latitude()))
        } else {
            self.constrained_min_scale()
        };
        let new_scale = mln_math::clamp_f64(zoom_scale(zoom), min_scale, self.max_scale);
        let new_world_size = new_scale * TILE_SIZE_D;
        let bc = new_world_size / constants::DEGREES_MAX;
        let cc = new_world_size / M2PI;

        // The globe's center can be on a pole, which the clamped sine stops
        // short of.
        let latitude = mln_math::deg2rad(constrained.latitude());
        let m = 1.0 - 1e-15;
        let f = mln_math::clamp_f64(latitude.sin(), -m, m);

        let x = -constrained.longitude() * bc;
        let y = if globe { cc * latitude.tan().asinh() } else { 0.5 * cc * ((1.0 + f) / (1.0 - f)).ln() };
        // A far unwrapped longitude can overflow the point.
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        let (scale, x, y) = self.constrain(new_scale, x, y);
        self.update(|p| {
            p.scale = scale;
            p.x = x;
            p.y = y;
        });
    }

    /// Sets the center's altitude, in meters.
    pub fn set_center_altitude(&mut self, alt_m: f64) {
        if !alt_m.is_finite() {
            return;
        }
        let view = &self.view;
        let z = alt_m
            / Projection::get_meters_per_pixel_at_latitude(view.center(WrapMode::Unwrapped).latitude(), view.zoom());
        self.update(|p| p.z = z);
    }
}

#[cfg(test)]
#[path = "transform_state_test.rs"]
mod transform_state_test;
