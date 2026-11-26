use std::f64::consts::PI;

use constants::TILE_SIZE_D;
use geometry::Point;
use glam::{DQuat, DVec3};
use mln_style::constants;
use mln_style::error::Error;
use mln_style::geo::{LatLng, LatLngBounds, WrapMode};
use mln_style::size::Size;
use mln_style::types::ProjectionDefinition;

use crate::camera::camera::{CameraOptions, FreeCameraOptions};
use crate::camera::camera_limits::CameraLimits;
use crate::camera::camera_pose::CameraPose;
use crate::camera::ground::GroundHit;
use crate::camera::mode::{ConstrainMode, ViewportMode};
use crate::camera::vertical_perspective_projection::VerticalPerspectiveProjection;
use crate::camera::view::{
    NorthOrientation, View, ViewParams, map_offset, max_mercator_horizon_angle, scale_zoom, zoom_scale,
};
use crate::geo::constants::M2PI;
use crate::geo::projection::Projection;
use crate::geo::{EdgeInsets, ScreenCoordinate};

fn lat_lng_from_mercator(mercator_coordinate: Point<f64>, wrap_mode: WrapMode) -> Result<LatLng, Error> {
    LatLng::with_mode(
        mln_style::math::rad2deg(2.0 * (PI - mercator_coordinate.y * 2.0 * PI).exp().atan() - (PI / 2.0)),
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
    limits: CameraLimits,
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
            limits: CameraLimits::default(),
        }
    }

    /// A state that shows `params` within `limits`, as they are.
    pub(crate) fn from_parts(params: ViewParams, limits: CameraLimits) -> Self {
        let mut state = TransformState { view: View::new(params), limits };
        state.update(|_| ());
        state
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
        params.center_wrap = if params.projection_transition > 0.0 && self.limits.bounds == LatLngBounds::default() {
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
                if self.limits.bounds == LatLngBounds::default() { WrapMode::Wrapped } else { WrapMode::Unwrapped };
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
        new_pitch = mln_style::math::clamp_f64(new_pitch, self.limits.min_pitch, self.limits.max_pitch);

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
            let new_scale = mln_style::math::clamp_f64(2.0f64.powf(zoom), self.min_scale(), self.max_scale());
            travel = center_distance / new_scale / TILE_SIZE_D;
        }

        let mercator_point = Point::new(position.x + dx * travel, position.y + dy * travel);
        let Ok(center) = lat_lng_from_mercator(mercator_point, WrapMode::Unwrapped) else {
            return;
        };
        let zoom = f64::min(zoom, self.limits.max_zoom);
        self.set_lat_lng_zoom(&center, zoom);

        let mercator_z = position.z + dz * travel;
        let alt_m = mercator_z
            * Projection::meters_per_pixel_at_latitude(self.view.center(WrapMode::Unwrapped).latitude(), 0.0)
            * TILE_SIZE_D;
        self.set_center_altitude(alt_m);
        self.set_angle(new_bearing);
        self.set_pitch(new_pitch);
        self.set_roll(new_roll);
    }

    /// `position` with its altitude clamped to the zoom range, or `None` for
    /// a position that isn't a number.
    fn constrained_camera_position(&self, position: DVec3) -> Option<DVec3> {
        if position.is_nan() {
            return None;
        }
        let max_world_size = Projection::world_size(2.0f64.powf(self.limits.max_zoom));
        let min_world_size = Projection::world_size(2.0f64.powf(self.min_zoom()));
        let dist_to_center = self.view.camera_to_center_distance();
        Some(DVec3::new(
            position.x,
            position.y,
            mln_style::math::clamp_f64(position.z, dist_to_center / max_world_size, dist_to_center / min_world_size),
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
        if !self.is_valid() {
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
    pub fn constrain_mode(&self) -> ConstrainMode {
        self.view.params().constrain_mode
    }

    pub fn set_constrain_mode(&mut self, mode: ConstrainMode) {
        self.update(|p| p.constrain_mode = mode);
    }

    pub fn set_viewport_mode(&mut self, mode: ViewportMode) {
        self.update(|p| p.viewport_mode = mode);
    }

    pub fn set_center_clamped_to_ground(&mut self, clamped: bool) {
        self.update(|p| p.center_clamped_to_ground = clamped);
    }

    pub fn set_globe_free_bearing(&mut self, free_bearing: bool) {
        self.update(|p| p.globe_free_bearing = free_bearing);
    }

    pub fn set_padding(&mut self, padding: &EdgeInsets) {
        self.update(|p| p.padding = *padding);
    }

    // MARK: - Camera options

    /// The camera, with `padding` in place of its own if given.
    #[must_use]
    pub fn camera_options(&self, padding: Option<EdgeInsets>) -> CameraOptions {
        let mut camera = self.view.camera();
        if let Some(padding) = padding {
            camera.padding = padding;
        }
        CameraOptions::from(camera)
    }

    // MARK: - Zoom

    /// Lowest zoom allowed with the map centered at this latitude: the globe
    /// lets the zoom fall below `min_zoom()` toward the poles by the
    /// anchored-zoom adjustment, so the planet keeps its size on screen.
    #[must_use]
    pub fn min_zoom_at_latitude(&self, latitude: f64) -> f64 {
        self.min_zoom_at_latitude_with(latitude, self.view.is_globe())
    }

    /// [`min_zoom_at_latitude`](Self::min_zoom_at_latitude) for a
    /// camera on the globe or not, whatever the current projection.
    #[must_use]
    pub fn min_zoom_at_latitude_with(&self, latitude: f64, globe: bool) -> f64 {
        if !globe {
            return self.min_zoom();
        }
        // The globe's constraint never raises the scale, whichever projection
        // the map is in now.
        scale_zoom(self.min_scale()) + VerticalPerspectiveProjection::zoom_adjustment(0.0, latitude)
    }

    /// The center `constrain` would settle on for this one at this zoom: on
    /// the globe the zoom decides how close to a pole the center can be.
    #[must_use]
    pub fn constrained_center(&self, lat_lng: &LatLng, zoom: f64) -> LatLng {
        if self.constrain_mode() == ConstrainMode::None || !self.view.is_globe() {
            return *lat_lng;
        }
        let max_latitude = VerticalPerspectiveProjection::max_latitude(
            &self.view,
            mln_style::math::clamp_f64(zoom, self.min_zoom_at_latitude(lat_lng.latitude()), self.limits.max_zoom),
        );
        LatLng::saturating(
            mln_style::math::clamp_f64(lat_lng.latitude(), -max_latitude, max_latitude),
            lat_lng.longitude(),
            WrapMode::Unwrapped,
        )
    }

    // MARK: - Limits

    /// The limits the camera moves within.
    #[must_use]
    pub fn limits(&self) -> &CameraLimits {
        &self.limits
    }

    /// Sets the limits the camera moves within, clamped to what any map
    /// allows (see [`CameraLimits::validated`]), and moves the center into
    /// new bounds. `Err`, changing nothing, for limits that leave the camera
    /// nowhere to go.
    pub fn set_limits(&mut self, limits: CameraLimits) -> Result<(), Error> {
        let limits = limits.validated()?;
        // The view can't zoom out past the zoom at which the world fills it.
        let least_scale = self.constrain(zoom_scale(limits.min_zoom), self.view.x(), self.view.y()).0;
        if zoom_scale(limits.max_zoom) < least_scale {
            return Err(Error::invalid_argument("the maximum zoom must be at least the least zoom the view allows"));
        }
        let bounds_changed = limits.bounds != self.limits.bounds;
        self.limits = limits;
        if bounds_changed {
            let lat_lng = self.view.center(WrapMode::Unwrapped);
            let zoom = self.view.zoom();
            self.set_lat_lng_zoom(&lat_lng, zoom);
            self.update(|_| ());
        }
        Ok(())
    }

    fn min_scale(&self) -> f64 {
        zoom_scale(self.limits.min_zoom)
    }

    fn max_scale(&self) -> f64 {
        zoom_scale(self.limits.max_zoom)
    }

    /// The lowest zoom the constrain mode allows: the minimum zoom, raised
    /// so that the view shows no area off the world.
    #[must_use]
    pub fn min_zoom(&self) -> f64 {
        scale_zoom(self.constrained_min_scale())
    }

    fn constrained_min_scale(&self) -> f64 {
        self.constrain(self.min_scale(), self.view.x(), self.view.y()).0
    }

    // MARK: - Rotation

    /// Sets the map's rotation, in radians counterclockwise: the negated
    /// bearing.
    pub fn set_angle(&mut self, angle: f64) {
        if angle.is_finite() {
            self.update(|p| p.angle = angle);
        }
    }

    pub fn set_fov(&mut self, fov: f64) {
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
    pub fn is_valid(&self) -> bool {
        let scale = self.view.scale();
        self.view.is_valid()
            && (if self.view.is_globe() { scale > 0.0 } else { scale >= self.min_scale() })
            && scale <= self.max_scale()
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
        if self.constrain_mode() != ConstrainMode::Screen {
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
        let constrain_mode = self.constrain_mode();
        if constrain_mode == ConstrainMode::None || constrain_mode == ConstrainMode::Screen {
            return (scale, x, y);
        }

        // The globe has no off-world area: the center goes past the Mercator
        // range toward a pole as far as the zoom allows.
        if self.view.is_globe() {
            let max_latitude = VerticalPerspectiveProjection::max_latitude(&self.view, scale_zoom(scale));
            let max_y = Projection::world_size(scale) / M2PI * mln_style::math::deg2rad(max_latitude).tan().asinh();
            y = mln_style::math::clamp_f64(y, -max_y, max_y);
            return (scale, x, y);
        }

        let size = self.view.size();
        let width = f64::from(size.width);
        let height = f64::from(size.height);

        // Constrain scale to avoid zooming out far enough to show off-world
        // areas on the Y axis.
        let ratio_y = (if self.rotated_north() { width } else { height }) / TILE_SIZE_D;
        scale = mln_style::math::fmax(scale, ratio_y);

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
        if self.constrain_mode() != ConstrainMode::Screen || self.limits.bounds == LatLngBounds::default() {
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

        let current_bounds = self.limits.bounds;
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
    pub fn min_zoom_for_screen_bounds(&self) -> Option<f64> {
        if self.constrain_mode() != ConstrainMode::Screen || self.limits.bounds == LatLngBounds::default() {
            return None;
        }
        // The bounds' extent on screen grows with the scale alone, so they
        // fill the screen at the same zoom wherever the center is.
        let ne = Projection::project(&self.limits.bounds.northeast(), 1.0);
        let sw = Projection::project(&self.limits.bounds.southwest(), 1.0);
        let size = self.view.size();
        let scale_x = f64::from(size.width) / (ne.x - sw.x);
        let scale_y = f64::from(size.height) / (sw.y - ne.y);
        Some(scale_zoom(f64::max(scale_x, scale_y)))
    }

    // MARK: - Moving the camera

    /// Recenter the map so that the given coordinate is located at the given
    /// point on screen.
    /// Fails with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for a
    /// NaN or infinite anchor.
    pub fn move_lat_lng(&mut self, lat_lng: &LatLng, anchor: &ScreenCoordinate) -> Result<(), Error> {
        self.move_lat_lng_at(lat_lng, anchor, 0.0)
    }

    /// Recenters the map so that `lat_lng`, `height` meters above sea level,
    /// is at `anchor`, a point measured from the bottom edge (y up): the
    /// center moves across the ground, with the zoom, the angles and the
    /// center's altitude kept. At height 0 it is
    /// [`move_lat_lng`](Self::move_lat_lng). The globe turns to hold
    /// `lat_lng` on the sphere `height` meters above sea level, keeping the
    /// planet's size on the screen. Fails as `move_lat_lng` does.
    pub fn move_lat_lng_at(&mut self, lat_lng: &LatLng, anchor: &ScreenCoordinate, height: f64) -> Result<(), Error> {
        let view = &self.view;
        if view.is_globe() {
            if let Some(center) =
                VerticalPerspectiveProjection::center_for_location_at_point(view, lat_lng, anchor, height)
            {
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
        let center = self.center_for_lat_lng_at(lat_lng, anchor, height)?;
        // Wrapped, as easeTo and GL JS's setLocationAtPoint leave it, unless
        // bounds keep it in their range.
        let target = if self.limits.bounds == LatLngBounds::default() { center.wrapped() } else { center };
        let zoom = self.view.zoom();
        self.set_lat_lng_zoom(&target, zoom);
        Ok(())
    }

    /// The center, unwrapped and before the limits constrain it, that puts
    /// `lat_lng`, `height` meters above sea level, at `anchor` (y up) on a
    /// Mercator view: the current center moved across the ground by as far
    /// as `lat_lng` is from the location the line of sight through `anchor`
    /// meets at that height (GL JS's `getCameraForLocationAtPoint`). On the
    /// globe it is the center that holds `lat_lng` on the sphere at that
    /// height, or fails where none does; before the globe has a size, its
    /// points are Mercator's, as
    /// [`crate::camera::View::screen_coordinate_to_lat_lng_at`] reads them. Fails as
    /// [`move_lat_lng_at`](Self::move_lat_lng_at) does.
    pub fn center_for_lat_lng_at(
        &self,
        lat_lng: &LatLng,
        anchor: &ScreenCoordinate,
        height: f64,
    ) -> Result<LatLng, Error> {
        let view = &self.view;
        if view.is_globe() && !view.size().is_empty() {
            return VerticalPerspectiveProjection::center_for_location_at_point(view, lat_lng, anchor, height)
                .ok_or_else(|| Error::invalid_argument("no center holds the location at the point"));
        }
        let scale = view.scale();
        let center_coord = Projection::project(&view.center(WrapMode::Unwrapped), scale);
        let lat_lng_coord = Projection::project(lat_lng, scale);
        let anchor_coord =
            Projection::project(&view.screen_coordinate_to_lat_lng_at(anchor, height, WrapMode::Unwrapped)?, scale);
        Projection::unproject(
            &Point::new(
                center_coord.x + lat_lng_coord.x - anchor_coord.x,
                center_coord.y + lat_lng_coord.y - anchor_coord.y,
            ),
            scale,
            WrapMode::Unwrapped,
        )
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
        if globe && self.limits.bounds != LatLngBounds::default() {
            let turns =
                ((self.limits.bounds.center().longitude() - lat_lng.longitude()) / constants::DEGREES_MAX).round();
            requested = LatLng::saturating(
                lat_lng.latitude(),
                lat_lng.longitude() + turns * constants::DEGREES_MAX,
                WrapMode::Unwrapped,
            );
        }
        let mut constrained = self.limits.bounds.constrain(&requested);
        // The globe's tile cover picks each tile's wrap from the center, so a
        // center that jumped a whole world at the antimeridian would re-key
        // every tile; keep it on the copy nearest the current one.
        if globe && self.limits.bounds == LatLngBounds::default() {
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
            zoom_scale(self.min_zoom_at_latitude(constrained.latitude()))
        } else {
            self.constrained_min_scale()
        };
        let new_scale = mln_style::math::clamp_f64(zoom_scale(zoom), min_scale, self.max_scale());
        let (x, y) = map_offset(&constrained, new_scale, globe);
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

    /// Sets the center's altitude, in meters, which then holds through
    /// changes of the zoom and the center.
    pub fn set_center_altitude(&mut self, alt_m: f64) {
        if !alt_m.is_finite() {
            return;
        }
        self.update(|p| p.center_altitude = alt_m);
    }

    /// Sets the lowest ground near the center while terrain draws, in
    /// meters, exaggerated, or `None` without terrain. With it, the view's
    /// near and far planes fit raised and sunken ground; without it, they
    /// fit the flat map. A height that isn't finite changes nothing.
    pub fn set_terrain_min_elevation(&mut self, min_elevation: Option<f64>) {
        if min_elevation.is_some_and(|min| !min.is_finite()) {
            return;
        }
        self.update(|p| p.terrain_min_elevation = min_elevation);
    }

    /// Raises the eye by `lift` meters straight up, still looking at the
    /// center at its altitude: keeps the center, its altitude, the bearing
    /// and the roll, and solves the pitch and the zoom that put the eye there
    /// (GL JS's `calculateCameraOptionsFromTo`, from the raised eye to the
    /// center), within the limits. Heights convert to pixels at the center's
    /// latitude. On the globe it solves in the plane of the center's
    /// vertical, which the eye is near wherever it can come near the
    /// ground. Returns whether the view changed; while the view shows
    /// nothing, it doesn't.
    pub fn raise_eye(&mut self, lift: f64) -> bool {
        let view = &self.view;
        if !lift.is_finite() || !view.is_valid() {
            return false;
        }
        // The eye's offset from the center, in pixels at the zoom: across
        // the ground, and up.
        let distance = view.camera_to_center_distance();
        let across = view.pitch().sin() * distance;
        let up = view.pitch().cos() * distance + lift / Self::height_meters_per_pixel(view);
        let raised = across.hypot(up);
        if raised == 0.0 {
            return false;
        }
        let pitch = mln_style::math::clamp_f64(across.atan2(up), self.limits.min_pitch, self.limits.max_pitch);
        let zoom = view.zoom() + (distance / raised).log2();
        let center = view.center(WrapMode::Unwrapped);
        let before = *view.params();
        self.set_pitch(pitch);
        self.set_lat_lng_zoom(&center, zoom);
        *self.view.params() != before
    }

    /// Moves the center along the view onto `hit`, the ground its line of
    /// sight meets, with the eye, the pitch and the bearing kept: the center
    /// stands as far along the view as `hit` is from the eye, at the height
    /// the view reaches there, and the zoom follows from that distance (GL
    /// JS's `moveCenterToElevation`, orbis's `recenter_on`). Distances across
    /// the ground convert to meters at `hit`'s latitude, so where the
    /// center's latitude was another, the center lands off `hit` by that
    /// share of the distance between the latitudes' scales.
    ///
    /// On the globe the center moves along the line from the eye through
    /// the center at its altitude, as far as `hit`, raised along its
    /// vertical, is from the eye along it; it stands that far above the
    /// sea-level sphere, and the zoom puts the eye that far from it (GL
    /// JS's `calculateCameraOptionsFromTo`). The pitch and the bearing are
    /// kept, which turns the eye about the new center by the angle between
    /// the centers' verticals: under a meter where the camera is low enough
    /// to meet terrain.
    ///
    /// Returns whether it moved the center there. It leaves the state as it
    /// is while the view shows nothing, where `hit` isn't in front of the
    /// eye, and where the limits would move the zoom or the center: the zoom
    /// range, the bounds, and in [`ConstrainMode::Screen`] the bounds the
    /// screen must stay within.
    pub fn recenter_on(&mut self, hit: &GroundHit) -> bool {
        let view = &self.view;
        if !view.is_valid() {
            return false;
        }
        let recentered =
            if view.is_globe() { Self::recentered_on_sphere(view, hit) } else { self.recentered_on_plane(hit) };
        let Some((center, altitude, zoom)) = recentered else { return false };

        let mut moved = self.clone();
        moved.set_center_altitude(altitude);
        moved.set_lat_lng_zoom(&center, zoom);
        // The limits clamp what they refuse, and the screen's bounds move it.
        let landed = moved.view.center(WrapMode::Unwrapped);
        let mut on_screen = CameraOptions { center: Some(landed), ..CameraOptions::default() };
        let mut screen_zoom = moved.view.zoom();
        moved.constrain_camera_and_zoom_to_bounds(&mut on_screen, &mut screen_zoom);
        let refused = (moved.view.zoom() - zoom).abs() > 1e-9
            || (landed.latitude() - center.latitude()).abs() > 1e-9
            || (landed.longitude() - center.longitude()).abs() > 1e-9
            || on_screen.center != Some(landed)
            || screen_zoom != moved.view.zoom();
        if refused {
            return false;
        }
        *self = moved;
        true
    }

    /// The center, its altitude and the zoom of [`Self::recenter_on`] on a
    /// Mercator view, before the limits.
    fn recentered_on_plane(&self, hit: &GroundHit) -> Option<(LatLng, f64, f64)> {
        let view = &self.view;
        let eye = view.eye_position();
        let forward = view.camera_pose().forward();
        // Meters per Web Mercator unit at zoom 0, across the ground at `hit`.
        let latitude = hit.location(WrapMode::Unwrapped).latitude();
        let meters = Projection::meters_per_pixel_at_latitude(latitude, 0.0) * TILE_SIZE_D;
        let to_hit = DVec3::new((hit.world.x - eye.x) * meters, (hit.world.y - eye.y) * meters, hit.height - eye.z);
        let distance = to_hit.dot(forward);
        if !(distance.is_finite() && distance > 0.0) {
            return None;
        }
        let world = Point::new(eye.x + forward.x * distance / meters, eye.y + forward.y * distance / meters);
        let center = Projection::unproject(&world, 1.0 / TILE_SIZE_D, WrapMode::Unwrapped).ok()?;
        // Wrapped, as move_lat_lng leaves it, unless bounds keep it in their
        // range.
        let center = if self.limits.bounds == LatLngBounds::default() { center.wrapped() } else { center };
        let zoom = scale_zoom(view.camera_to_center_distance() * meters / distance / TILE_SIZE_D);
        Some((center, eye.z + forward.z * distance, zoom))
    }

    /// The center, its altitude and the zoom of [`Self::recenter_on`] on the
    /// globe, before the limits, in unit-sphere space. The center keeps the
    /// world copy nearest the current one, as the globe's moves do.
    fn recentered_on_sphere(view: &View, hit: &GroundHit) -> Option<(LatLng, f64, f64)> {
        let radius = |height: f64| 1.0 + height / VerticalPerspectiveProjection::GLOBE_RADIUS_METERS;
        let eye = view.globe_camera_position();
        let center = VerticalPerspectiveProjection::surface_vector(&view.center(WrapMode::Unwrapped))
            * radius(view.center_altitude());
        let forward = (center - eye).normalize();
        let target =
            VerticalPerspectiveProjection::surface_vector(&hit.location(WrapMode::Unwrapped)) * radius(hit.height);
        let distance = (target - eye).dot(forward);
        if !(distance.is_finite() && distance > 0.0) {
            return None;
        }
        let point = eye + forward * distance;
        let length = point.length();
        let location = VerticalPerspectiveProjection::surface_vector_to_lat_lng(point / length)?;
        let location = view.lat_lng_near_center(&location);
        let radius_at_zoom_0 =
            VerticalPerspectiveProjection::globe_radius_pixels(Projection::world_size(1.0), location.latitude());
        let zoom = scale_zoom(view.camera_to_center_distance() / distance / radius_at_zoom_0);
        Some((location, (length - 1.0) * VerticalPerspectiveProjection::GLOBE_RADIUS_METERS, zoom))
    }

    /// Meters per pixel at the zoom of a height at the center: on Mercator
    /// the view's, and on the globe those of its sphere, which it raises
    /// heights on.
    fn height_meters_per_pixel(view: &View) -> f64 {
        if view.is_globe() {
            let latitude = view.center(WrapMode::Unwrapped).latitude();
            VerticalPerspectiveProjection::GLOBE_RADIUS_METERS
                / VerticalPerspectiveProjection::globe_radius_pixels(Projection::world_size(view.scale()), latitude)
        } else {
            view.meters_per_pixel()
        }
    }
}

#[cfg(test)]
#[path = "transform_state_test.rs"]
mod transform_state_test;
