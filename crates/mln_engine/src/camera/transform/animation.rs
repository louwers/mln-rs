//! Animated camera commands: easing and flying to a camera.

use std::f64::consts::PI;
use std::time::Duration;

use mln_style::geo::{LatLngBounds, WrapMode};
use mln_style::interpolate::interpolate;

use super::transition::{AlongFn, FrameFn};
use super::{
    CameraCommandId, Transform, frame_edge_insets, frame_unproject, lat_lng, nanos_from_secs, normalize_angle,
};
use crate::camera::{
    AnimationOptions, CameraOptions, TransformState, TransitionOutcome, VerticalPerspectiveProjection,
};
use crate::geo::EdgeInsets;
use crate::geo::projection::Projection;

/// An animation frame's padding, `t` of the way from `start` to `end`; `None`
/// when it has none (see [`frame_edge_insets`]). Insets that don't change are
/// kept as they are, so that infinite insets don't log on every frame.
fn interpolate_edge_insets(start: &EdgeInsets, end: &EdgeInsets, t: f64) -> Option<EdgeInsets> {
    if start == end {
        return Some(*end);
    }
    frame_edge_insets(
        interpolate(&start.top, &end.top, t),
        interpolate(&start.left, &end.left, t),
        interpolate(&start.bottom, &end.bottom, t),
        interpolate(&start.right, &end.right, t),
    )
}

impl Transform {
    /// Change any combination of center, zoom, bearing, pitch and edgeInsets,
    /// with a smooth animation between old and new values. Omitted options
    /// keep their current animations and timing.
    pub fn ease_to(&mut self, camera: &CameraOptions, animation: &AnimationOptions) -> CameraCommandId {
        self.command(|transform| transform.ease(camera, animation))
    }

    pub(super) fn ease(&mut self, input_camera: &CameraOptions, animation: &AnimationOptions) {
        let mut camera = *input_camera;
        let globe = self.state.view().is_globe();

        let duration = animation.duration.unwrap_or(Duration::ZERO);
        if !globe
            && self.state.limits().bounds == LatLngBounds::default()
            && !self.is_gesture_in_progress()
            && duration != Duration::ZERO
            && animation.easing.is_none()
        {
            // A plain timed ease flies without the zoom-out. The flight's
            // ground speed rises and falls along its path even at a constant
            // zoom, so an ease given an easing interpolates instead, and
            // moves as that easing says, as GL JS's easeTo does.
            self.fly_to(&camera, animation, true);
            return;
        }

        let mut zoom = camera.zoom.unwrap_or(self.view().zoom());
        if camera.zoom.is_none()
            && let Some(center) = camera.center
            && globe
        {
            zoom += VerticalPerspectiveProjection::zoom_adjustment(
                self.view().center(WrapMode::Wrapped).latitude(),
                center.latitude(),
            );
        }
        self.state.constrain_camera_and_zoom_to_bounds(&mut camera, &mut zoom);

        let padding = camera.padding.unwrap_or(self.state.view().padding());
        let mut start_lat_lng = self.view().center(WrapMode::Unwrapped);
        let unwrapped_lat_lng = camera.center.unwrap_or(start_lat_lng);
        // On the globe the ease heads for the center the zoom it was given
        // allows, or else the zoom it starts from, as GL JS's does; its last
        // frame is bounded by the zoom it ends on.
        let lat_lng_end = self.state.constrained_center(
            &if self.state.limits().bounds == LatLngBounds::default() {
                unwrapped_lat_lng.wrapped()
            } else {
                unwrapped_lat_lng
            },
            if camera.zoom.is_some() { zoom } else { self.view().zoom() },
        );

        let mut bearing = match camera.bearing {
            Some(b) => mln_style::math::deg2rad(-b),
            None => self.view().angle(),
        };
        let mut pitch = match camera.pitch {
            Some(p) => mln_style::math::deg2rad(p),
            None => self.view().pitch(),
        };
        let mut fov = match camera.fov {
            Some(f) => mln_style::math::deg2rad(f),
            None => self.view().fov(),
        };
        let center_alt = camera.center_altitude.unwrap_or(self.state.view().center_altitude());
        let roll = match camera.roll {
            Some(r) => mln_style::math::deg2rad(r),
            None => self.view().roll(),
        };

        if zoom.is_nan() || bearing.is_nan() || pitch.is_nan() || roll.is_nan() || fov.is_nan() {
            self.end_unstarted(animation, TransitionOutcome::Cancelled);
            return;
        }

        if self.state.limits().bounds == LatLngBounds::default() {
            if self.is_gesture_in_progress() {
                // If gesture in progress, we transfer the wrap rounds from the
                // end longitude into start, so the "scroll effect" of rounding
                // the world is the same while assuring the end longitude
                // remains wrapped.
                let wrap = unwrapped_lat_lng.longitude() - lat_lng_end.longitude();
                start_lat_lng = lat_lng(start_lat_lng.latitude(), start_lat_lng.longitude() - wrap);
            } else {
                // Find the shortest path otherwise.
                start_lat_lng.unwrap_for_shortest_path(&lat_lng_end);
            }
        }
        let start_center_alt = self.state.view().center_altitude();

        let start_scale = self.state.view().scale();
        let start_point = Projection::project(&start_lat_lng, start_scale);
        let end_point = Projection::project(&lat_lng_end, start_scale);

        // Constrain camera options.
        // The target is drawn in the projection the style gives at its zoom,
        // which the animation may cross into.
        let target_globe = match &self.projection_for_zoom {
            Some(projection_for_zoom) => projection_for_zoom(zoom).transition_state() > 0.0,
            None => globe,
        };
        zoom = mln_style::math::clamp_f64(
            zoom,
            self.state.min_zoom_at_latitude_with(lat_lng_end.latitude(), target_globe),
            self.state.limits().max_zoom,
        );
        pitch = mln_style::math::clamp_f64(pitch, self.state.limits().min_pitch, self.state.limits().max_pitch);
        fov = mln_style::math::clamp_f64(fov, crate::camera::MIN_FIELD_OF_VIEW, crate::camera::MAX_FIELD_OF_VIEW);

        // Minimize rotation by taking the shorter path around the circle.
        bearing = normalize_angle(bearing, self.state.view().angle());

        let start_zoom = self.state.view().zoom();
        let start_bearing = normalize_angle(self.state.view().angle(), bearing);
        let start_pitch = self.state.view().pitch();
        let start_roll = self.state.view().roll();
        let start_fov = self.state.view().fov();
        let start_edge_insets = self.state.view().padding();

        // On the globe the zoom is eased in equator terms so the planet holds
        // its size as the latitude changes, and the center moves the way GL
        // JS moves it: longitude paced at 1 / cos(latitude), sped up ahead of
        // a zoom in.
        let normalized_start_zoom = if globe {
            start_zoom + VerticalPerspectiveProjection::zoom_adjustment(start_lat_lng.latitude(), 0.0)
        } else {
            start_zoom
        };
        let normalized_zoom = if globe {
            zoom + VerticalPerspectiveProjection::zoom_adjustment(lat_lng_end.latitude(), 0.0)
        } else {
            zoom
        };
        let final_scale = crate::camera::zoom_scale(normalized_zoom - normalized_start_zoom);
        let base = if normalized_zoom > normalized_start_zoom {
            f64::min(2.0, final_scale)
        } else {
            f64::max(0.5, final_scale)
        };
        let delta_latitude = if globe {
            VerticalPerspectiveProjection::difference_of_angles_degrees(
                start_lat_lng.latitude(),
                lat_lng_end.latitude(),
            )
        } else {
            0.0
        };
        let delta_longitude = if globe {
            VerticalPerspectiveProjection::difference_of_angles_degrees(
                start_lat_lng.longitude(),
                lat_lng_end.longitude(),
            )
        } else {
            0.0
        };
        let moves_around_anchor = camera.anchor.is_some() && camera.center.is_none();

        let frame: FrameFn = Box::new(move |state: &TransformState, t: f64| {
            let (frame_lat_lng, frame_zoom) = if globe {
                // An anchor moves the center a frame at a time (see
                // apply_transitions), so it stays, and the zoom follows its
                // latitude to keep the planet's size.
                let mut frame_lat_lng =
                    if moves_around_anchor { state.view().center(WrapMode::Unwrapped) } else { lat_lng_end };
                let mut frame_zoom = zoom;
                if moves_around_anchor {
                    frame_zoom = interpolate(&normalized_start_zoom, &normalized_zoom, t)
                        + VerticalPerspectiveProjection::zoom_adjustment(0.0, frame_lat_lng.latitude());
                } else if t < 1.0 {
                    let factor = t * base.powf(1.0 - t);
                    frame_lat_lng = VerticalPerspectiveProjection::interpolate_lat_lng(
                        &start_lat_lng,
                        delta_latitude,
                        delta_longitude,
                        factor,
                    )
                    .wrapped();
                    frame_zoom = if zoom == start_zoom {
                        start_zoom
                    } else {
                        interpolate(&normalized_start_zoom, &normalized_zoom, t)
                            + VerticalPerspectiveProjection::zoom_adjustment(0.0, frame_lat_lng.latitude())
                    };
                }
                (Some(frame_lat_lng), frame_zoom)
            } else if t == 1.0 {
                (Some(lat_lng_end), zoom)
            } else {
                let frame_point = interpolate(&start_point, &end_point, t);
                (frame_unproject(&frame_point, start_scale, WrapMode::Unwrapped), interpolate(&start_zoom, &zoom, t))
            };
            CameraOptions {
                center: frame_lat_lng,
                zoom: Some(frame_zoom),
                center_altitude: Some(interpolate(&start_center_alt, &center_alt, t)),
                bearing: Some(-mln_style::math::rad2deg(mln_style::math::wrap(
                    interpolate(&start_bearing, &bearing, t),
                    -PI,
                    PI,
                ))),
                padding: interpolate_edge_insets(&start_edge_insets, &padding, t),
                pitch: Some(mln_style::math::rad2deg(interpolate(&start_pitch, &pitch, t))),
                roll: Some(mln_style::math::rad2deg(interpolate(&start_roll, &roll, t))),
                fov: Some(mln_style::math::rad2deg(interpolate(&start_fov, &fov, t))),
                ..Default::default()
            }
        });

        // On the globe the zoom keeps the planet's size as the center's
        // latitude changes, and a zoom moves the center when it bounds it
        // closer to the equator, so an ease that moves the center sets both
        // (an anchored one gets its center from the anchor).
        let moves_center = input_camera.center.is_some() || lat_lng_end.wrapped() != start_lat_lng.wrapped();
        let fields = if globe && moves_around_anchor {
            CameraOptions { zoom: Some(zoom), ..*input_camera }
        } else if globe && moves_center {
            CameraOptions { center: Some(lat_lng_end), zoom: Some(zoom), ..*input_camera }
        } else {
            *input_camera
        };
        self.start_transition(&fields, animation, frame, None, duration, false);
    }

    /// This method implements an “optimal path” animation, as detailed in:
    ///
    /// Van Wijk, Jarke J.; Nuij, Wim A. A. “Smooth and efficient zooming and
    /// panning.” INFOVIS ’03. pp. 15–22.
    /// <https://www.win.tue.nl/~vanwijk/zoompan.pdf#page=5>.
    ///
    /// Where applicable, local variable documentation begins with the
    /// associated variable or function in van Wijk (2003).
    pub(crate) fn fly_to(
        &mut self,
        camera: &CameraOptions,
        animation: &AnimationOptions,
        linear_zoom_interpolation: bool,
    ) -> CameraCommandId {
        self.command(|transform| transform.fly(camera, animation, linear_zoom_interpolation))
    }

    pub(super) fn fly(
        &mut self,
        input_camera: &CameraOptions,
        animation: &AnimationOptions,
        linear_zoom_interpolation: bool,
    ) {
        let mut camera = *input_camera;
        let globe = self.state.view().is_globe();

        let mut zoom = camera.zoom.unwrap_or(self.view().zoom());
        if camera.zoom.is_none()
            && let Some(center) = camera.center
            && globe
        {
            zoom += VerticalPerspectiveProjection::zoom_adjustment(
                self.view().center(WrapMode::Wrapped).latitude(),
                center.latitude(),
            );
        }
        self.state.constrain_camera_and_zoom_to_bounds(&mut camera, &mut zoom);

        let padding = camera.padding.unwrap_or(self.state.view().padding());
        // As in easeTo, the globe's flight heads for the center the zoom it
        // was given, or else starts from, allows.
        let lat_lng_end = self.state.constrained_center(
            &camera.center.unwrap_or(self.view().center(WrapMode::Unwrapped)).wrapped(),
            if camera.zoom.is_some() { zoom } else { self.view().zoom() },
        );
        let center_alt = camera.center_altitude.unwrap_or(self.state.view().center_altitude());
        let mut bearing = match camera.bearing {
            Some(b) => mln_style::math::deg2rad(-b),
            None => self.view().angle(),
        };
        let mut pitch = match camera.pitch {
            Some(p) => mln_style::math::deg2rad(p),
            None => self.view().pitch(),
        };
        let roll = match camera.roll {
            Some(r) => mln_style::math::deg2rad(r),
            None => self.view().roll(),
        };
        let mut fov = match camera.fov {
            Some(f) => mln_style::math::deg2rad(f),
            None => self.view().fov(),
        };

        if zoom.is_nan()
            || bearing.is_nan()
            || pitch.is_nan()
            || roll.is_nan()
            || fov.is_nan()
            || self.state.view().size().is_empty()
        {
            self.end_unstarted(animation, TransitionOutcome::Cancelled);
            return;
        }

        // Determine endpoints.
        let mut start_lat_lng = self.view().center(WrapMode::Unwrapped).wrapped();
        start_lat_lng.unwrap_for_shortest_path(&lat_lng_end);
        let start_center_alt = self.state.view().center_altitude();

        let start_point = Projection::project(&start_lat_lng, self.state.view().scale());
        let end_point = Projection::project(&lat_lng_end, self.state.view().scale());

        // Constrain camera options.
        // The target is drawn in the projection the style gives at its zoom,
        // which the animation may cross into.
        let target_globe = match &self.projection_for_zoom {
            Some(projection_for_zoom) => projection_for_zoom(zoom).transition_state() > 0.0,
            None => globe,
        };
        zoom = mln_style::math::clamp_f64(
            zoom,
            self.state.min_zoom_at_latitude_with(lat_lng_end.latitude(), target_globe),
            self.state.limits().max_zoom,
        );
        pitch = mln_style::math::clamp_f64(pitch, self.state.limits().min_pitch, self.state.limits().max_pitch);
        fov = mln_style::math::clamp_f64(fov, crate::camera::MIN_FIELD_OF_VIEW, crate::camera::MAX_FIELD_OF_VIEW);

        // Minimize rotation by taking the shorter path around the circle.
        bearing = normalize_angle(bearing, self.state.view().angle());
        let start_zoom = crate::camera::scale_zoom(self.state.view().scale());
        let start_bearing = normalize_angle(self.state.view().angle(), bearing);
        let start_pitch = self.state.view().pitch();
        let start_roll = self.state.view().roll();
        let start_fov = self.state.view().fov();

        // w₀: Initial visible span, measured in pixels at the initial scale.
        // Known henceforth as a <i>screenful</i>.
        let size = self.state.view().size();
        let w0 = f64::max(
            f64::from(size.width) - padding.left - padding.right,
            f64::from(size.height) - padding.top - padding.bottom,
        );
        // The globe counts zoom in equator terms and distance along the
        // surface, so the arc is the same at any latitude.
        let normalized_start_zoom = if globe {
            start_zoom + VerticalPerspectiveProjection::zoom_adjustment(start_lat_lng.latitude(), 0.0)
        } else {
            start_zoom
        };
        let normalized_zoom = if globe {
            zoom + VerticalPerspectiveProjection::zoom_adjustment(lat_lng_end.latitude(), 0.0)
        } else {
            zoom
        };
        // w₁: Final visible span, measured in pixels with respect to the
        // initial scale.
        let w1 = w0 / crate::camera::zoom_scale(normalized_zoom - normalized_start_zoom);
        // Length of the flight path as projected onto the ground plane,
        // measured in pixels from the world image origin at the initial scale.
        let u1 = if globe {
            VerticalPerspectiveProjection::surface_distance_pixels(
                Projection::world_size(self.state.view().scale()),
                start_lat_lng.latitude(),
                &start_lat_lng,
                &lat_lng_end,
            )
        } else {
            (end_point.x - start_point.x).hypot(end_point.y - start_point.y)
        };

        // ρ: The relative amount of zooming that takes place along the flight
        // path. A high value maximizes zooming for an exaggerated animation,
        // while a low value minimizes zooming for something closer to
        // easeTo().
        //
        // 1.42 is the average value selected by participants in the user
        // study in van Wijk (2003). A value of 6<sup>¼</sup> would be
        // equivalent to the root mean squared average velocity,
        // V<sub>RMS</sub>. A value of 1 produces a circular motion.
        let mut rho = 1.42;
        if globe {
            // GL JS: the arc may not dip below the map's or the flight's
            // minimum zoom, and that only ever lowers ρ.
            let floor_zoom = f64::max(animation.min_zoom.unwrap_or(self.state.min_zoom()), self.state.min_zoom());
            let normalized_floor_zoom = f64::min(
                f64::min(
                    floor_zoom + VerticalPerspectiveProjection::zoom_adjustment(lat_lng_end.latitude(), 0.0),
                    normalized_start_zoom,
                ),
                normalized_zoom,
            );
            let min_zoom = mln_style::math::clamp_f64(
                normalized_floor_zoom + VerticalPerspectiveProjection::zoom_adjustment(0.0, lat_lng_end.latitude()),
                self.state.min_zoom_at_latitude(lat_lng_end.latitude()),
                self.state.limits().max_zoom,
            );
            let w_max = w0
                / crate::camera::zoom_scale(
                    min_zoom + VerticalPerspectiveProjection::zoom_adjustment(lat_lng_end.latitude(), 0.0)
                        - normalized_start_zoom,
                );
            if u1 != 0.0 {
                rho = f64::min(rho, (w_max / u1 * 2.0).sqrt());
            }
        } else if animation.min_zoom.is_some() || linear_zoom_interpolation {
            // util::min(a, b, c) = min(a, min(b, c)), fmin on doubles
            let mut min_zoom = mln_style::math::fmin(
                animation.min_zoom.unwrap_or(start_zoom),
                mln_style::math::fmin(start_zoom, zoom),
            );
            min_zoom = mln_style::math::clamp_f64(
                min_zoom,
                self.state.min_zoom_at_latitude(lat_lng_end.latitude()),
                self.state.limits().max_zoom,
            );
            // w<sub>m</sub>: Maximum visible span, measured in pixels with
            // respect to the initial scale.
            let w_max = w0 / crate::camera::zoom_scale(min_zoom - start_zoom);
            rho = if u1 == 0.0 { 1.0 } else { (w_max / u1 * 2.0).sqrt() };
        }
        // ρ²
        let rho2 = rho * rho;

        // rᵢ: Returns the zoom-out factor at one end of the animation.
        //
        // @param i 0 for the ascent or 1 for the descent.
        let r = |i: f64| -> f64 {
            // bᵢ
            let b = (w1 * w1 - w0 * w0 + if i == 0.0 { 1.0 } else { -1.0 } * rho2 * rho2 * u1 * u1)
                / (2.0 * if i == 0.0 { w0 } else { w1 } * rho2 * u1);
            ((b * b + 1.0).sqrt() - b).ln()
        };

        // r₀: Zoom-out factor during ascent.
        let r0 = if u1 == 0.0 { f64::INFINITY } else { r(0.0) }; // Silence division by 0 on sanitize bot.
        let r1 = if u1 == 0.0 { f64::INFINITY } else { r(1.0) };

        // When u₀ = u₁, the optimal path doesn’t require both ascent and
        // descent.
        let is_close = u1.abs() < 0.000_001 || !r0.is_finite() || !r1.is_finite();

        // w(s): Returns the visible span on the ground, measured in pixels
        // with respect to the initial scale.
        //
        // Assumes an angular field of view of 2 arctan ½ ≈ 53°.
        let w = move |s: f64| -> f64 {
            if is_close {
                ((if w1 < w0 { -1.0 } else { 1.0 }) * rho * s).exp()
            } else {
                r0.cosh() / (r0 + rho * s).cosh()
            }
        };
        // u(s): Returns the distance along the flight path as projected onto
        // the ground plane, measured in pixels from the world image origin at
        // the initial scale.
        let u = move |s: f64| -> f64 {
            if is_close { 0.0 } else { w0 * (r0.cosh() * (r0 + rho * s).tanh() - r0.sinh()) / rho2 / u1 }
        };
        // S: Total length of the flight path, measured in ρ-screenfuls.
        let total_s = if is_close { (w1 / w0).ln().abs() / rho } else { (r1 - r0) / rho };
        // Where the center stays, the share of the time stands in.
        let along: Option<AlongFn> = if is_close { None } else { Some(Box::new(move |k| u(k * total_s))) };

        let duration = if let Some(duration) = animation.duration {
            duration
        } else {
            // V: Average velocity, measured in ρ-screenfuls per second.
            let mut velocity = 1.2;
            if let Some(v) = animation.velocity {
                velocity = v / rho;
            }
            // A duration that a `Duration` can't hold jumps, as one past
            // max_duration does, rather than saturating to a flight that
            // never ends.
            nanos_from_secs(total_s / velocity).unwrap_or_default()
        };
        let duration = match animation.max_duration {
            Some(max) if !linear_zoom_interpolation && duration > max => Duration::ZERO,
            _ => duration,
        };
        let start_scale = self.state.view().scale();
        let start_edge_insets = self.state.view().padding();
        let delta_latitude = if globe {
            VerticalPerspectiveProjection::difference_of_angles_degrees(
                start_lat_lng.latitude(),
                lat_lng_end.latitude(),
            )
        } else {
            0.0
        };
        let delta_longitude = if globe {
            VerticalPerspectiveProjection::difference_of_angles_degrees(
                start_lat_lng.longitude(),
                lat_lng_end.longitude(),
            )
        } else {
            0.0
        };

        let frame: FrameFn = Box::new(move |_: &TransformState, k: f64| {
            // s: The distance traveled along the flight path, measured in
            // ρ-screenfuls.
            let s = k * total_s;
            let us = if k == 1.0 { 1.0 } else { u(s) };

            let (frame_lat_lng, frame_zoom) = if globe {
                let mut frame_lat_lng = lat_lng_end;
                let mut frame_zoom = zoom;
                if k < 1.0 {
                    frame_lat_lng = VerticalPerspectiveProjection::interpolate_lat_lng(
                        &start_lat_lng,
                        delta_latitude,
                        delta_longitude,
                        us,
                    )
                    .wrapped();
                    let frame_normalized_zoom = if linear_zoom_interpolation {
                        interpolate(&normalized_start_zoom, &normalized_zoom, k)
                    } else {
                        normalized_start_zoom + crate::camera::scale_zoom(1.0 / w(s))
                    };
                    frame_zoom = frame_normalized_zoom
                        + VerticalPerspectiveProjection::zoom_adjustment(0.0, frame_lat_lng.latitude());
                    if frame_zoom.is_nan() {
                        frame_zoom = zoom;
                    }
                }
                (Some(frame_lat_lng), frame_zoom)
            } else {
                // Calculate the current point and zoom level along the flight
                // path.
                let frame_point = interpolate(&start_point, &end_point, us);
                let mut frame_zoom = if linear_zoom_interpolation {
                    interpolate(&start_zoom, &zoom, k)
                } else {
                    start_zoom + crate::camera::scale_zoom(1.0 / w(s))
                };

                // Zoom can be NaN if size is empty.
                if frame_zoom.is_nan() {
                    frame_zoom = zoom;
                }

                // Convert to geographic coordinates and set the new viewpoint.
                (frame_unproject(&frame_point, start_scale, WrapMode::Unwrapped), frame_zoom)
            };
            CameraOptions {
                center: frame_lat_lng,
                zoom: Some(frame_zoom),
                center_altitude: Some(interpolate(&start_center_alt, &center_alt, us)),
                bearing: Some(-mln_style::math::rad2deg(mln_style::math::wrap(
                    interpolate(&start_bearing, &bearing, k),
                    -PI,
                    PI,
                ))),
                padding: interpolate_edge_insets(&start_edge_insets, &padding, k),
                pitch: Some(mln_style::math::rad2deg(interpolate(&start_pitch, &pitch, k))),
                roll: Some(mln_style::math::rad2deg(interpolate(&start_roll, &roll, k))),
                fov: Some(mln_style::math::rad2deg(interpolate(&start_fov, &fov, k))),
                ..Default::default()
            }
        });

        self.start_transition(input_camera, animation, frame, along, duration, !linear_zoom_interpolation);
    }
}
