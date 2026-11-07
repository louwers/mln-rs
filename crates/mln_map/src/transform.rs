// Ports src/mln/map/transform.hpp and src/mln/map/transform.cpp.
//
// The C++ transition lambdas capture `this` and mutate `state`. Here the
// stored transition functions take `&mut Transform` as an argument; like
// C++ `updateTransitions`, they are moved out of the Transform before they
// are called.

use std::f64::consts::PI;
use std::rc::Rc;

use geometry::Point;
use mln_map_types::{
    AnimationOptions, CameraOptions, CenterAndBearing, ConstrainMode, FreeCameraOptions, NorthOrientation,
    ProjectionMode, TransformState, TransformStateProperties, VerticalPerspectiveProjection, ViewportMode,
};
use mln_style_types::ProjectionDefinition;
use mln_util::chrono::{Clock, Duration, TimePoint};
use mln_util::constants::{DEFAULT_TRANSITION_EASE, M2PI, PITCH_MAX};
use mln_util::geo::{EdgeInsets, LatLng, LatLngBounds, ScreenCoordinate, WrapMode};
use mln_util::interpolate::interpolate;
use mln_util::logging::{Event, Log};
use mln_util::projection::Projection;
use mln_util::size::Size;

use crate::map_observer::CameraChangeMode;

/// `std::min`.
fn std_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// `std::max`.
fn std_max(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

/// Converts the given angle (in radians) to be numerically close to the
/// anchor angle, allowing it to be interpolated properly without sudden
/// jumps.
fn normalize_angle(mut angle: f64, anchor_angle: f64) -> f64 {
    if angle.is_nan() || anchor_angle.is_nan() {
        return 0.0;
    }

    angle = mln_math::wrap(angle, -PI, PI);
    if angle == -PI {
        angle = PI;
    }
    let diff = (angle - anchor_angle).abs();
    if (angle - M2PI - anchor_angle).abs() < diff {
        angle -= M2PI;
    }
    if (angle + M2PI - anchor_angle).abs() < diff {
        angle += M2PI;
    }

    angle
}

/// `util::angle_between` (util/math.hpp).
fn angle_between(a: &Point<f64>, b: &Point<f64>) -> f64 {
    (a.x * b.y - a.y * b.x).atan2(a.x * b.x + a.y * b.y)
}

fn lat_lng(lat: f64, lon: f64) -> LatLng {
    // C++ constructs LatLng directly, which throws std::domain_error.
    LatLng::new(lat, lon).unwrap_or_else(|e| panic!("{e}"))
}

fn edge_insets(top: f64, left: f64, bottom: f64, right: f64) -> EdgeInsets {
    // C++ constructs EdgeInsets directly, which throws std::domain_error.
    EdgeInsets::new(top, left, bottom, right).unwrap_or_else(|e| panic!("{e}"))
}

/// `Projection::unproject`, which constructs a LatLng and so can throw
/// std::domain_error; nothing catches it on these paths.
fn unproject(p: &Point<f64>, scale: f64, wrap_mode: WrapMode) -> LatLng {
    Projection::unproject(p, scale, wrap_mode).unwrap_or_else(|e| panic!("{e}"))
}

/// `std::to_string(double)`: `%f`.
fn std_to_string(value: f64) -> String {
    format!("{value:.6}")
}

/// `TransformObserver` (src/mln/map/transform.hpp).
pub trait TransformObserver {
    fn on_camera_will_change(&self, _mode: CameraChangeMode) {}
    fn on_camera_is_changing(&self) {}
    fn on_camera_did_change(&self, _mode: CameraChangeMode) {}
}

/// `TransformObserver::nullObserver()`.
pub struct NullTransformObserver;
impl TransformObserver for NullTransformObserver {}

type FrameFn = Rc<dyn Fn(&mut TransformState, f64)>;
type TransitionFrameFn = Box<dyn FnMut(&mut Transform, TimePoint) -> bool>;
type TransitionFinishFn = Box<dyn FnMut(&mut Transform)>;
/// `std::function<ProjectionDefinition(double zoom)>`.
pub type ProjectionForZoomFn = Rc<dyn Fn(f64) -> ProjectionDefinition>;

pub struct Transform {
    observer: Rc<dyn TransformObserver>,
    state: TransformState,

    transition_start: TimePoint,
    transition_duration: Duration,
    transition_frame_fn: Option<TransitionFrameFn>,
    transition_finish_fn: Option<TransitionFinishFn>,
    projection_for_zoom: Option<ProjectionForZoomFn>,
    globe_free_bearing: bool,
}

impl Transform {
    pub fn new(
        observer: Rc<dyn TransformObserver>,
        constrain_mode: ConstrainMode,
        viewport_mode: ViewportMode,
    ) -> Self {
        Transform {
            observer,
            state: TransformState::new(constrain_mode, viewport_mode),
            transition_start: TimePoint::default(),
            transition_duration: Duration::zero(),
            transition_frame_fn: None,
            transition_finish_fn: None,
            projection_for_zoom: None,
            globe_free_bearing: false,
        }
    }

    /// `Transform(const TransformState&)`.
    pub fn from_state(state: TransformState) -> Self {
        Transform {
            observer: Rc::new(NullTransformObserver),
            state,
            transition_start: TimePoint::default(),
            transition_duration: Duration::zero(),
            transition_frame_fn: None,
            transition_finish_fn: None,
            projection_for_zoom: None,
            globe_free_bearing: false,
        }
    }

    // MARK: - Map View

    pub fn resize(&mut self, size: Size) -> Result<(), String> {
        if size.is_empty() {
            return Err("failed to resize: size is empty".to_owned());
        }

        if self.state.get_size() == size {
            return Ok(());
        }

        self.observer.on_camera_will_change(CameraChangeMode::Immediate);
        self.state.set_size(size);
        let mut scale = self.state.get_scale();
        let mut x = self.state.get_x();
        let mut y = self.state.get_y();
        let z = self.state.get_z();

        let mut lat = 0.0;
        let mut lon = 0.0;
        if self.state.constrain_screen(&mut scale, &mut lat, &mut lon) {
            // Turns out that if you resize during a transition any changes
            // made to the state will be ignored :( So if we have to constrain
            // because of a resize and a transition is in progress - cancel
            // the transition!
            if self.in_transition() {
                self.cancel_transitions();
            }

            // It also turns out that state.setProperties isn't enough if you
            // change the center, you need to set Cc and Bc too, which
            // setLatLngZoom does.
            let zoom = self.state.scale_zoom(scale);
            self.state.set_lat_lng_zoom(&lat_lng(lat, lon), zoom);
            self.observer.on_camera_did_change(CameraChangeMode::Immediate);

            return Ok(());
        }
        self.state.constrain(&mut scale, &mut x, &mut y);
        self.state.set_properties(
            &TransformStateProperties::default()
                .with_scale(Some(scale))
                .with_x(Some(x))
                .with_y(Some(y))
                .with_z(Some(z)),
        );

        self.observer.on_camera_did_change(CameraChangeMode::Immediate);
        Ok(())
    }

    // MARK: - Camera

    /// Returns the current camera options.
    pub fn get_camera_options(&self, padding: Option<EdgeInsets>) -> CameraOptions {
        self.state.get_camera_options(padding)
    }

    /// Change any combination of center, zoom, bearing, and pitch, without a
    /// transition. The map will retain the current values for any options
    /// not included in `options`.
    pub fn jump_to(&mut self, camera: &CameraOptions) {
        // On the globe the zoom decides how close to a pole the center can be.
        // GL JS's jump bounds the center by the zoom it lands on, where its
        // ease bounds it by the zoom it starts from unless given one, so the
        // jump settles the zoom.
        if self.state.is_globe_rendering()
            && let Some(center) = camera.center
            && camera.zoom.is_none()
        {
            let zoom = self.get_zoom()
                + VerticalPerspectiveProjection::zoom_adjustment(
                    self.get_lat_lng(WrapMode::Wrapped).latitude(),
                    center.latitude(),
                );
            self.ease_to(&camera.with_zoom(Some(zoom)), &AnimationOptions::default());
            return;
        }
        self.ease_to(camera, &AnimationOptions::default());
    }

    /// Change any combination of center, zoom, bearing, pitch and edgeInsets,
    /// with a smooth animation between old and new values. The map will
    /// retain the current values for any options not included in `options`.
    pub fn ease_to(&mut self, input_camera: &CameraOptions, animation: &AnimationOptions) {
        let mut camera = *input_camera;
        let globe = self.state.is_globe_rendering();

        let duration = animation.duration.unwrap_or(Duration::zero());
        if !globe
            && self.state.get_lat_lng_bounds() == LatLngBounds::default()
            && !self.is_gesture_in_progress()
            && duration != Duration::zero()
        {
            // reuse flyTo, without exaggerated animation, to achieve constant
            // ground speed.
            self.fly_to(&camera, animation, true);
            return;
        }

        let mut zoom = camera.zoom.unwrap_or(self.get_zoom());
        if camera.zoom.is_none()
            && let Some(center) = camera.center
            && globe
        {
            zoom += VerticalPerspectiveProjection::zoom_adjustment(
                self.get_lat_lng(WrapMode::Wrapped).latitude(),
                center.latitude(),
            );
        }
        self.state.constrain_camera_and_zoom_to_bounds(&mut camera, &mut zoom);

        let padding = camera.padding.unwrap_or(self.state.get_edge_insets());
        let mut start_lat_lng = self.get_lat_lng(WrapMode::Unwrapped);
        let unwrapped_lat_lng = camera.center.unwrap_or(start_lat_lng);
        // On the globe the ease heads for the center the zoom it was given
        // allows, or else the zoom it starts from, as GL JS's does; its last
        // frame is bounded by the zoom it ends on.
        let lat_lng_end = self.state.constrained_center(
            &if self.state.get_lat_lng_bounds() != LatLngBounds::default() {
                unwrapped_lat_lng
            } else {
                unwrapped_lat_lng.wrapped()
            },
            if camera.zoom.is_some() { zoom } else { self.get_zoom() },
        );

        let mut bearing = match camera.bearing {
            Some(b) => mln_math::deg2rad(-b),
            None => self.get_bearing(),
        };
        let mut pitch = match camera.pitch {
            Some(p) => mln_math::deg2rad(p),
            None => self.get_pitch(),
        };
        let mut fov = match camera.fov {
            Some(f) => mln_math::deg2rad(f),
            None => self.get_field_of_view(),
        };
        let center_alt = camera.center_altitude.unwrap_or(self.state.get_center_altitude());
        let roll = match camera.roll {
            Some(r) => mln_math::deg2rad(r),
            None => self.get_roll(),
        };

        if zoom.is_nan() || bearing.is_nan() || pitch.is_nan() || roll.is_nan() || fov.is_nan() {
            if let Some(finish) = &animation.transition_finish_fn {
                finish();
            }
            return;
        }

        if self.state.get_lat_lng_bounds() == LatLngBounds::default() {
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
        let start_center_alt = self.state.get_center_altitude();

        let start_point = Projection::project(&start_lat_lng, self.state.get_scale());
        let end_point = Projection::project(&lat_lng_end, self.state.get_scale());

        // Constrain camera options.
        // The target is drawn in the projection the style gives at its zoom,
        // which the animation may cross into.
        let target_globe = match &self.projection_for_zoom {
            Some(projection_for_zoom) => projection_for_zoom(zoom).transition_state() > 0.0,
            None => globe,
        };
        zoom = mln_math::clamp_f64(
            zoom,
            self.state.get_min_zoom_at_latitude_with(lat_lng_end.latitude(), target_globe),
            self.state.get_max_zoom(),
        );
        pitch = mln_math::clamp_f64(pitch, self.state.get_min_pitch(), self.state.get_max_pitch());
        fov = mln_math::clamp_f64(fov, self.state.get_min_field_of_view(), self.state.get_max_field_of_view());

        // Minimize rotation by taking the shorter path around the circle.
        bearing = normalize_angle(bearing, self.state.get_bearing());
        let normalized = normalize_angle(self.state.get_bearing(), bearing);
        self.state.set_bearing(normalized);

        let start_zoom = self.state.get_zoom();
        let start_bearing = self.state.get_bearing();
        let start_pitch = self.state.get_pitch();
        let start_roll = self.state.get_roll();
        let start_fov = self.state.get_field_of_view() as f64;
        self.state.set_properties(
            &TransformStateProperties::default()
                .with_panning_in_progress(Some(unwrapped_lat_lng != start_lat_lng))
                .with_scaling_in_progress(Some(zoom != start_zoom))
                .with_rotating_in_progress(Some(bearing != start_bearing)),
        );
        let start_edge_insets = self.state.get_edge_insets();

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
        let final_scale = self.state.zoom_scale(normalized_zoom - normalized_start_zoom);
        let base =
            if normalized_zoom > normalized_start_zoom { std_min(2.0, final_scale) } else { std_max(0.5, final_scale) };
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

        let frame: FrameFn = Rc::new(move |state: &mut TransformState, t: f64| {
            if globe {
                // An anchor moves the center a frame at a time (see
                // start_transition), so it stays, and the zoom follows its
                // latitude to keep the planet's size.
                let mut frame_lat_lng =
                    if moves_around_anchor { state.get_lat_lng(WrapMode::Unwrapped) } else { lat_lng_end };
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
                    frame_zoom = if zoom != start_zoom {
                        interpolate(&normalized_start_zoom, &normalized_zoom, t)
                            + VerticalPerspectiveProjection::zoom_adjustment(0.0, frame_lat_lng.latitude())
                    } else {
                        start_zoom
                    };
                }
                state.set_lat_lng_zoom(&frame_lat_lng, frame_zoom);
            } else {
                let frame_point = interpolate(&start_point, &end_point, t);
                let frame_lat_lng = unproject(&frame_point, state.zoom_scale(start_zoom), WrapMode::Unwrapped);
                let frame_zoom = interpolate(&start_zoom, &zoom, t);
                state.set_lat_lng_zoom(&frame_lat_lng, frame_zoom);
            }
            state.set_center_altitude(interpolate(&start_center_alt, &center_alt, t));
            if bearing != start_bearing {
                state.set_bearing(mln_math::wrap(interpolate(&start_bearing, &bearing, t), -PI, PI));
            }
            if padding != start_edge_insets {
                // Interpolate edge insets. C++ goes through the EdgeInsets
                // constructor, which throws std::domain_error on NaN (from
                // infinite insets); nothing catches it.
                state.set_edge_insets(&edge_insets(
                    interpolate(&start_edge_insets.top, &padding.top, t),
                    interpolate(&start_edge_insets.left, &padding.left, t),
                    interpolate(&start_edge_insets.bottom, &padding.bottom, t),
                    interpolate(&start_edge_insets.right, &padding.right, t),
                ));
            }
            let max_pitch = get_max_pitch_for_edge_insets(state, &state.get_edge_insets());
            if pitch != start_pitch || max_pitch < start_pitch {
                state.set_pitch(std_min(max_pitch, interpolate(&start_pitch, &pitch, t)));
            }
            if roll != start_roll {
                state.set_roll(interpolate(&start_roll, &roll, t));
            }
            if fov != start_fov {
                state.set_field_of_view(interpolate(&start_fov, &fov, t));
            }
        });

        self.start_transition(&camera, animation, frame, duration);
    }

    /// This method implements an “optimal path” animation, as detailed in:
    ///
    /// Van Wijk, Jarke J.; Nuij, Wim A. A. “Smooth and efficient zooming and
    /// panning.” INFOVIS ’03. pp. 15–22.
    /// <https://www.win.tue.nl/~vanwijk/zoompan.pdf#page=5>.
    ///
    /// Where applicable, local variable documentation begins with the
    /// associated variable or function in van Wijk (2003).
    pub fn fly_to(
        &mut self,
        input_camera: &CameraOptions,
        animation: &AnimationOptions,
        linear_zoom_interpolation: bool,
    ) {
        let mut camera = *input_camera;
        let globe = self.state.is_globe_rendering();

        let mut zoom = camera.zoom.unwrap_or(self.get_zoom());
        if camera.zoom.is_none()
            && let Some(center) = camera.center
            && globe
        {
            zoom += VerticalPerspectiveProjection::zoom_adjustment(
                self.get_lat_lng(WrapMode::Wrapped).latitude(),
                center.latitude(),
            );
        }
        self.state.constrain_camera_and_zoom_to_bounds(&mut camera, &mut zoom);

        let padding = camera.padding.unwrap_or(self.state.get_edge_insets());
        // As in easeTo, the globe's flight heads for the center the zoom it
        // was given, or else starts from, allows.
        let lat_lng_end = self.state.constrained_center(
            &camera.center.unwrap_or(self.get_lat_lng(WrapMode::Unwrapped)).wrapped(),
            if camera.zoom.is_some() { zoom } else { self.get_zoom() },
        );
        let center_alt = camera.center_altitude.unwrap_or(self.state.get_center_altitude());
        let mut bearing = match camera.bearing {
            Some(b) => mln_math::deg2rad(-b),
            None => self.get_bearing(),
        };
        let mut pitch = match camera.pitch {
            Some(p) => mln_math::deg2rad(p),
            None => self.get_pitch(),
        };
        let roll = match camera.roll {
            Some(r) => mln_math::deg2rad(r),
            None => self.get_roll(),
        };
        let mut fov = match camera.fov {
            Some(f) => mln_math::deg2rad(f),
            None => self.get_field_of_view(),
        };

        if zoom.is_nan()
            || bearing.is_nan()
            || pitch.is_nan()
            || roll.is_nan()
            || fov.is_nan()
            || self.state.get_size().is_empty()
        {
            if let Some(finish) = &animation.transition_finish_fn {
                finish();
            }
            return;
        }

        // Determine endpoints.
        let mut start_lat_lng = self.get_lat_lng(WrapMode::Unwrapped).wrapped();
        start_lat_lng.unwrap_for_shortest_path(&lat_lng_end);
        let start_center_alt = self.state.get_center_altitude();

        let start_point = Projection::project(&start_lat_lng, self.state.get_scale());
        let end_point = Projection::project(&lat_lng_end, self.state.get_scale());

        // Constrain camera options.
        // The target is drawn in the projection the style gives at its zoom,
        // which the animation may cross into.
        let target_globe = match &self.projection_for_zoom {
            Some(projection_for_zoom) => projection_for_zoom(zoom).transition_state() > 0.0,
            None => globe,
        };
        zoom = mln_math::clamp_f64(
            zoom,
            self.state.get_min_zoom_at_latitude_with(lat_lng_end.latitude(), target_globe),
            self.state.get_max_zoom(),
        );
        pitch = mln_math::clamp_f64(pitch, self.state.get_min_pitch(), self.state.get_max_pitch());
        fov = mln_math::clamp_f64(fov, self.state.get_min_field_of_view(), self.state.get_max_field_of_view());

        // Minimize rotation by taking the shorter path around the circle.
        bearing = normalize_angle(bearing, self.state.get_bearing());
        let normalized = normalize_angle(self.state.get_bearing(), bearing);
        self.state.set_bearing(normalized);
        let start_zoom = self.state.scale_zoom(self.state.get_scale());
        let start_bearing = self.state.get_bearing();
        let start_pitch = self.state.get_pitch();
        let start_roll = self.state.get_roll();
        let start_fov = self.state.get_field_of_view() as f64;

        // w₀: Initial visible span, measured in pixels at the initial scale.
        // Known henceforth as a <i>screenful</i>.
        let size = self.state.get_size();
        let w0 = std_max(
            size.width as f64 - padding.left - padding.right,
            size.height as f64 - padding.top - padding.bottom,
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
        let w1 = w0 / self.state.zoom_scale(normalized_zoom - normalized_start_zoom);
        // Length of the flight path as projected onto the ground plane,
        // measured in pixels from the world image origin at the initial scale.
        let u1 = if globe {
            VerticalPerspectiveProjection::surface_distance_pixels(
                Projection::world_size(self.state.get_scale()),
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
            let floor_zoom =
                std_max(animation.min_zoom.unwrap_or(self.state.get_min_zoom()), self.state.get_min_zoom());
            // std::min({a, b, c}): the first of the smallest.
            let normalized_floor_zoom = std_min(
                std_min(
                    floor_zoom + VerticalPerspectiveProjection::zoom_adjustment(lat_lng_end.latitude(), 0.0),
                    normalized_start_zoom,
                ),
                normalized_zoom,
            );
            let min_zoom = mln_math::clamp_f64(
                normalized_floor_zoom + VerticalPerspectiveProjection::zoom_adjustment(0.0, lat_lng_end.latitude()),
                self.state.get_min_zoom_at_latitude(lat_lng_end.latitude()),
                self.state.get_max_zoom(),
            );
            let w_max = w0
                / self.state.zoom_scale(
                    min_zoom + VerticalPerspectiveProjection::zoom_adjustment(lat_lng_end.latitude(), 0.0)
                        - normalized_start_zoom,
                );
            if u1 != 0.0 {
                rho = std_min(rho, (w_max / u1 * 2.0).sqrt());
            }
        } else if animation.min_zoom.is_some() || linear_zoom_interpolation {
            // util::min(a, b, c) = min(a, min(b, c)), fmin on doubles
            let mut min_zoom =
                mln_math::min_f64(animation.min_zoom.unwrap_or(start_zoom), mln_math::min_f64(start_zoom, zoom));
            min_zoom = mln_math::clamp_f64(
                min_zoom,
                self.state.get_min_zoom_at_latitude(lat_lng_end.latitude()),
                self.state.get_max_zoom(),
            );
            // w<sub>m</sub>: Maximum visible span, measured in pixels with
            // respect to the initial scale.
            let w_max = w0 / self.state.zoom_scale(min_zoom - start_zoom);
            rho = if u1 != 0.0 { (w_max / u1 * 2.0).sqrt() } else { 1.0 };
        }
        // ρ²
        let rho2 = rho * rho;

        // rᵢ: Returns the zoom-out factor at one end of the animation.
        //
        // @param i 0 for the ascent or 1 for the descent.
        let r = |i: f64| -> f64 {
            // bᵢ
            let b = (w1 * w1 - w0 * w0 + (if i != 0.0 { -1.0 } else { 1.0 }) * rho2 * rho2 * u1 * u1)
                / (2.0 * (if i != 0.0 { w1 } else { w0 }) * rho2 * u1);
            ((b * b + 1.0).sqrt() - b).ln()
        };

        // r₀: Zoom-out factor during ascent.
        let r0 = if u1 != 0.0 { r(0.0) } else { f64::INFINITY }; // Silence division by 0 on sanitize bot.
        let r1 = if u1 != 0.0 { r(1.0) } else { f64::INFINITY };

        // When u₀ = u₁, the optimal path doesn’t require both ascent and
        // descent.
        let is_close = u1.abs() < 0.000001 || !r0.is_finite() || !r1.is_finite();

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

        let duration = if let Some(duration) = animation.duration {
            duration
        } else {
            // V: Average velocity, measured in ρ-screenfuls per second.
            let mut velocity = 1.2;
            if let Some(v) = animation.velocity {
                velocity = v / rho;
            }
            // duration_cast<Duration>(duration<double>(S / velocity))
            Duration::from_nanos((total_s / velocity * 1_000_000_000.0) as i64)
        };
        if duration == Duration::zero() {
            // Perform an instantaneous transition.
            self.jump_to(&camera);
            if let Some(finish) = &animation.transition_finish_fn {
                finish();
            }
            return;
        }

        let start_scale = self.state.get_scale();
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
        self.state.set_properties(
            &TransformStateProperties::default()
                .with_panning_in_progress(Some(true))
                .with_scaling_in_progress(Some(true))
                .with_rotating_in_progress(Some(bearing != start_bearing)),
        );
        let start_edge_insets = self.state.get_edge_insets();

        let frame: FrameFn = Rc::new(move |state: &mut TransformState, k: f64| {
            // s: The distance traveled along the flight path, measured in
            // ρ-screenfuls.
            let s = k * total_s;
            let us = if k == 1.0 { 1.0 } else { u(s) };

            if globe {
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
                        normalized_start_zoom + state.scale_zoom(1.0 / w(s))
                    };
                    frame_zoom = frame_normalized_zoom
                        + VerticalPerspectiveProjection::zoom_adjustment(0.0, frame_lat_lng.latitude());
                    if frame_zoom.is_nan() {
                        frame_zoom = zoom;
                    }
                }
                state.set_lat_lng_zoom(&frame_lat_lng, frame_zoom);
            } else {
                // Calculate the current point and zoom level along the flight
                // path.
                let frame_point = interpolate(&start_point, &end_point, us);
                let mut frame_zoom = if linear_zoom_interpolation {
                    interpolate(&start_zoom, &zoom, k)
                } else {
                    start_zoom + state.scale_zoom(1.0 / w(s))
                };

                // Zoom can be NaN if size is empty.
                if frame_zoom.is_nan() {
                    frame_zoom = zoom;
                }

                // Convert to geographic coordinates and set the new viewpoint.
                let frame_lat_lng = unproject(&frame_point, start_scale, WrapMode::Unwrapped);
                state.set_lat_lng_zoom(&frame_lat_lng, frame_zoom);
            }
            state.set_center_altitude(interpolate(&start_center_alt, &center_alt, us));
            if bearing != start_bearing {
                state.set_bearing(mln_math::wrap(interpolate(&start_bearing, &bearing, k), -PI, PI));
            }

            if padding != start_edge_insets {
                // Interpolate edge insets (throwing constructor, as above)
                state.set_edge_insets(&edge_insets(
                    interpolate(&start_edge_insets.top, &padding.top, k),
                    interpolate(&start_edge_insets.left, &padding.left, k),
                    interpolate(&start_edge_insets.bottom, &padding.bottom, k),
                    interpolate(&start_edge_insets.right, &padding.right, k),
                ));
            }

            if pitch != start_pitch {
                state.set_pitch(interpolate(&start_pitch, &pitch, k));
            }
            if roll != start_roll {
                // C++ sets the pitch here (sic).
                state.set_pitch(interpolate(&start_roll, &roll, k));
            }
            if fov != start_fov {
                state.set_field_of_view(interpolate(&start_fov, &fov, k));
            }
        });

        self.start_transition(&camera, animation, frame, duration);
    }

    // MARK: - Position

    /// Pans the map by the given amount.
    pub fn move_by(&mut self, offset: &ScreenCoordinate, animation: &AnimationOptions) {
        self.move_by_with_anchor(offset, None, animation);
    }

    /// `moveBy(offset, anchor, animation)`: a drag on the globe turns the
    /// planet so that the location under `anchor` follows the cursor.
    pub fn move_by_with_anchor(
        &mut self,
        offset: &ScreenCoordinate,
        anchor: Option<ScreenCoordinate>,
        animation: &AnimationOptions,
    ) {
        let flipped = |state: &TransformState, point: &ScreenCoordinate| {
            ScreenCoordinate::new(point.x, f64::from(state.get_size().height) - point.y)
        };
        let size = self.state.get_size();
        let view_center = self.state.get_edge_insets().get_center(size.width as u16, size.height as u16);
        let fling_duration = animation.duration.filter(|duration| *duration > Duration::zero());
        if self.state.is_globe_rendering()
            && let Some(duration) = fling_duration
            && self.globe_free_bearing
        {
            // A fling with the bearing free turns the planet on about the axis
            // the drag turned it about, so it carries on over a pole instead
            // of spinning round it.
            let start = CenterAndBearing {
                center: self.get_lat_lng(WrapMode::Unwrapped),
                bearing: -mln_math::rad2deg(self.get_bearing()),
            };
            let end = VerticalPerspectiveProjection::fling_camera(
                &self.state,
                &flipped(&self.state, &view_center),
                &ScreenCoordinate::new(offset.x, -offset.y),
            );
            let start_zoom = self.get_zoom();
            self.state.set_properties(
                &TransformStateProperties::default()
                    .with_panning_in_progress(Some(true))
                    .with_rotating_in_progress(Some(true)),
            );
            let frame: FrameFn = Rc::new(move |state: &mut TransformState, t: f64| {
                let camera = VerticalPerspectiveProjection::interpolate_camera(&start, &end, t);
                let center = state.constrained_center(&camera.center, state.get_zoom());
                state.set_lat_lng_zoom(
                    &center,
                    start_zoom
                        + VerticalPerspectiveProjection::zoom_adjustment(start.center.latitude(), center.latitude()),
                );
                state.set_bearing(-mln_math::deg2rad(camera.bearing));
            });
            self.start_transition(&CameraOptions::default(), animation, frame, duration);
            return;
        }

        if self.state.is_globe_rendering() && fling_duration.is_some() {
            // An animated move, a fling, turns the globe as GL JS's inertia
            // does: the location under the far end of a long one can be off
            // the planet or over a pole. It stops short of half a turn, which
            // the ease would take the other way round.
            let mut center = self
                .state
                .constrained_center(&VerticalPerspectiveProjection::pan_center(&self.state, offset), self.get_zoom());
            let longitude = self.get_lat_lng(WrapMode::Unwrapped).longitude();
            if (center.longitude() - longitude).abs() > 180.0 {
                center = lat_lng(center.latitude(), longitude + 179.5f64.copysign(center.longitude() - longitude));
            }
            self.ease_to(&CameraOptions::default().with_center(Some(center)), animation);
            return;
        }

        if self.state.is_globe_rendering() {
            // A drag turns the globe as GL JS's does: one rotation that brings
            // the location under the cursor back under it with the bearing
            // held, around the cursor when it is on the planet and around the
            // center otherwise. With a free bearing the rotation turns the
            // bearing too, so the location stays under the cursor.
            let grab = match anchor {
                Some(anchor)
                    if VerticalPerspectiveProjection::screen_coordinate_hits_globe(
                        &self.state,
                        &flipped(&self.state, &anchor),
                    ) =>
                {
                    anchor
                }
                _ => view_center,
            };
            let pan_delta = ScreenCoordinate::new(offset.x, -offset.y);
            let mut camera = CameraOptions::default();
            let mut center = if self.globe_free_bearing {
                let drag =
                    VerticalPerspectiveProjection::drag_camera(&self.state, &flipped(&self.state, &grab), &pan_delta);
                camera = camera.with_bearing(Some(drag.bearing));
                drag.center
            } else {
                VerticalPerspectiveProjection::drag_center(&self.state, &flipped(&self.state, &grab), &pan_delta)
            };
            center = self.state.constrained_center(&center, self.get_zoom());
            let zoom = self.get_zoom()
                + VerticalPerspectiveProjection::zoom_adjustment(
                    self.get_lat_lng(WrapMode::Wrapped).latitude(),
                    center.latitude(),
                );
            self.ease_to(&camera.with_center(Some(center)).with_zoom(Some(zoom)), animation);
            return;
        }

        let mut center_offset = *offset;

        // Reduce the offset so that it never goes past the horizon. If it
        // goes past the horizon, the pan direction is opposite of the
        // intended direction.
        let pitch = self.state.get_pitch();
        let offset_length = offset.x.hypot(offset.y);
        if pitch > 0.0 && offset_length > 0.0 {
            let camera_to_center =
                0.5 * self.state.get_size().height as f64 / (self.state.get_field_of_view() as f64 / 2.0).tan();
            let pixels_to_horizon = (camera_to_center / pitch.tan()).abs();
            // must be < 1 to keep the offset short of the horizon
            let horizon_factor = 0.75;
            let scale = pixels_to_horizon * horizon_factor / offset_length;
            if scale < 1.0 {
                center_offset = ScreenCoordinate::new(offset.x * scale, offset.y * scale);
            }
        }

        let size = self.state.get_size();
        let center = self.state.get_edge_insets().get_center(size.width as u16, size.height as u16);
        let point_on_screen = ScreenCoordinate::new(center.x - center_offset.x, center.y - center_offset.y);
        // Use unwrapped LatLng to carry information about moveBy direction.
        let center = self.screen_coordinate_to_lat_lng(&point_on_screen, WrapMode::Unwrapped);
        self.ease_to(&CameraOptions::default().with_center(Some(center)), animation);
    }

    /// `getLatLng(LatLng::WrapMode = LatLng::Wrapped)`.
    pub fn get_lat_lng(&self, wrap: WrapMode) -> LatLng {
        self.state.get_lat_lng(wrap)
    }

    // MARK: - Zoom

    pub fn get_zoom(&self) -> f64 {
        self.state.get_zoom()
    }

    // MARK: - Bounds

    pub fn set_lat_lng_bounds(&mut self, bounds: LatLngBounds) -> Result<(), String> {
        if !bounds.valid() {
            return Err("failed to set bounds: bounds are invalid".to_owned());
        }
        self.state.set_lat_lng_bounds(bounds);
        Ok(())
    }

    pub fn set_min_zoom(&mut self, min_zoom: f64) {
        if min_zoom.is_nan() {
            return;
        }
        self.state.set_min_zoom(min_zoom);
    }

    pub fn set_max_zoom(&mut self, max_zoom: f64) {
        if max_zoom.is_nan() {
            return;
        }
        self.state.set_max_zoom(max_zoom);
    }

    pub fn set_min_pitch(&mut self, min_pitch: f64) {
        if min_pitch.is_nan() {
            return;
        }
        if mln_math::deg2rad(min_pitch) < mln_util::constants::PITCH_MIN {
            Log::warning(
                Event::General,
                format!(
                    "Trying to set minimum pitch below the limit ({} degrees), the value will be clamped.",
                    std_to_string(mln_math::rad2deg(mln_util::constants::PITCH_MIN))
                ),
            );
        }
        self.state.set_min_pitch(mln_math::deg2rad(min_pitch));
    }

    pub fn set_max_pitch(&mut self, max_pitch: f64) {
        if max_pitch.is_nan() {
            return;
        }
        if mln_math::deg2rad(max_pitch) > PITCH_MAX {
            Log::warning(
                Event::General,
                format!(
                    "Trying to set maximum pitch above the limit ({} degrees), the value will be clamped.",
                    std_to_string(mln_math::rad2deg(PITCH_MAX))
                ),
            );
        }
        self.state.set_max_pitch(mln_math::deg2rad(max_pitch));
    }

    pub fn set_frustum_offset(&mut self, frustum_offset: &EdgeInsets) {
        self.state.set_frustum_offset(frustum_offset);
    }

    pub fn get_frustum_offset(&self) -> EdgeInsets {
        self.state.get_frustum_offset()
    }

    // MARK: - Bearing

    pub fn rotate_by(&mut self, first: &ScreenCoordinate, second: &ScreenCoordinate, animation: &AnimationOptions) {
        let size = self.state.get_size();
        let mut center = self.state.get_edge_insets().get_center(size.width as u16, size.height as u16);
        let offset = ScreenCoordinate::new(first.x - center.x, first.y - center.y);
        // C++: std::sqrt(std::pow(2, offset.x) + std::pow(2, offset.y)) (sic).
        let distance = (2.0f64.powf(offset.x) + 2.0f64.powf(offset.y)).sqrt();

        // If the first click was too close to the center, move the center of
        // rotation by 200 pixels in the direction of the click.
        if distance < 200.0 {
            let height_offset = -200.0;
            let rotate_bearing = offset.y.atan2(offset.x);
            center.x = first.x + rotate_bearing.cos() * height_offset;
            center.y = first.y + rotate_bearing.sin() * height_offset;
        }

        let bearing = -mln_math::rad2deg(
            self.state.get_bearing()
                + angle_between(
                    &Point::new(first.x - center.x, first.y - center.y),
                    &Point::new(second.x - center.x, second.y - center.y),
                ),
        );
        self.ease_to(&CameraOptions::default().with_bearing(Some(bearing)), animation);
    }

    pub fn get_bearing(&self) -> f64 {
        self.state.get_bearing()
    }

    // MARK: - Pitch

    pub fn get_pitch(&self) -> f64 {
        self.state.get_pitch()
    }

    pub fn get_roll(&self) -> f64 {
        self.state.get_roll()
    }

    /// Returns `state.getFieldOfView()` (a float) as double.
    pub fn get_field_of_view(&self) -> f64 {
        self.state.get_field_of_view() as f64
    }

    // MARK: - Projection

    pub fn set_projection_definition(&mut self, definition: &ProjectionDefinition) {
        self.state.set_projection_definition(definition);
    }

    /// The projection the style gives at a zoom, which constrains an
    /// animation's target as it constrains a jump.
    pub fn set_projection_for_zoom(&mut self, projection_for_zoom: Option<ProjectionForZoomFn>) {
        self.projection_for_zoom = projection_for_zoom;
    }

    pub fn set_globe_free_bearing(&mut self, free_bearing: bool) {
        self.globe_free_bearing = free_bearing;
    }

    pub fn is_globe_free_bearing(&self) -> bool {
        self.globe_free_bearing
    }

    pub fn is_location_occluded(&self, lat_lng: &LatLng) -> bool {
        self.state.is_location_occluded(lat_lng)
    }

    pub fn set_north_orientation(&mut self, orientation: NorthOrientation) {
        self.state.set_north_orientation(orientation);
        self.constrain_and_set_properties();
    }

    pub fn get_north_orientation(&self) -> NorthOrientation {
        self.state.get_north_orientation()
    }

    fn constrain_and_set_properties(&mut self) {
        let mut scale = self.state.get_scale();
        let mut x = self.state.get_x();
        let mut y = self.state.get_y();
        let z = self.state.get_z();
        self.state.constrain(&mut scale, &mut x, &mut y);
        self.state.set_properties(
            &TransformStateProperties::default()
                .with_scale(Some(scale))
                .with_x(Some(x))
                .with_y(Some(y))
                .with_z(Some(z)),
        );
    }

    // MARK: - Constrain mode

    pub fn set_constrain_mode(&mut self, mode: ConstrainMode) {
        self.state.set_constrain_mode(mode);
        self.constrain_and_set_properties();
    }

    pub fn get_constrain_mode(&self) -> ConstrainMode {
        self.state.get_constrain_mode()
    }

    // MARK: - Viewport mode

    pub fn set_viewport_mode(&mut self, mode: ViewportMode) {
        self.state.set_viewport_mode(mode);
    }

    pub fn get_viewport_mode(&self) -> ViewportMode {
        self.state.get_viewport_mode()
    }

    // MARK: - Projection mode

    pub fn set_projection_mode(&mut self, options: &ProjectionMode) {
        let properties = TransformStateProperties::default()
            .with_axonometric(Some(options.axonometric.unwrap_or(self.state.get_axonometric())))
            .with_x_skew(Some(options.x_skew.unwrap_or(self.state.get_x_skew())))
            .with_y_skew(Some(options.y_skew.unwrap_or(self.state.get_y_skew())));
        self.state.set_properties(&properties);
    }

    pub fn get_projection_mode(&self) -> ProjectionMode {
        ProjectionMode::default()
            .with_axonometric(self.state.get_axonometric())
            .with_x_skew(self.state.get_x_skew())
            .with_y_skew(self.state.get_y_skew())
    }

    // MARK: - Transition

    fn start_transition(
        &mut self,
        camera: &CameraOptions,
        animation: &AnimationOptions,
        frame: FrameFn,
        duration: Duration,
    ) {
        if let Some(mut finish) = self.transition_finish_fn.take() {
            // C++ calls the stored std::function in place; it is overwritten
            // below.
            finish(self);
        }

        let is_animated = duration != Duration::zero();
        self.observer.on_camera_will_change(if is_animated {
            CameraChangeMode::Animated
        } else {
            CameraChangeMode::Immediate
        });

        // Associate the anchor, if given, with a coordinate. Anchor and
        // center points are mutually exclusive, with preference for the
        // center point when both are set.
        let mut anchor = if camera.center.is_some() { None } else { camera.anchor };
        let mut anchor_lat_lng = lat_lng(0.0, 0.0);
        if let Some(a) = anchor.as_mut() {
            a.y = self.state.get_size().height as f64 - a.y;
            anchor_lat_lng = self.state.screen_coordinate_to_lat_lng(a, WrapMode::Unwrapped);
        }

        self.transition_start = Clock::now();
        self.transition_duration = duration;

        let frame_animation = animation.clone();
        self.transition_frame_fn = Some(Box::new(move |transform: &mut Transform, now: TimePoint| -> bool {
            let t: f32 = if is_animated {
                Duration::float_seconds_div(now - transform.transition_start, transform.transition_duration)
            } else {
                1.0
            };
            // On the globe the anchor is re-read every frame and the zoom
            // around it is GL JS's blend of the exact solution and a damped
            // heuristic; the exact solution alone jumps near the horizon.
            let globe_zoom_around = anchor.is_some() && transform.state.is_globe_rendering();
            let zoom_location = match anchor {
                Some(anchor) if globe_zoom_around => {
                    transform.state.screen_coordinate_to_lat_lng(&anchor, WrapMode::Unwrapped)
                }
                _ => anchor_lat_lng,
            };
            let zoom_before = transform.state.get_zoom();
            if t >= 1.0 {
                frame(&mut transform.state, 1.0);
            } else {
                let ease = frame_animation.easing.unwrap_or(DEFAULT_TRANSITION_EASE);
                frame(&mut transform.state, ease.solve(t as f64, 0.001));
            }

            if let Some(anchor) = anchor {
                if globe_zoom_around && transform.state.get_zoom() != zoom_before {
                    let zoom_delta = transform.state.get_zoom() - zoom_before;
                    VerticalPerspectiveProjection::zoom_around_point(
                        &mut transform.state,
                        &anchor,
                        &zoom_location,
                        zoom_delta,
                        transform.globe_free_bearing,
                    );
                } else {
                    transform.state.move_lat_lng(&anchor_lat_lng, &anchor);
                }
            }

            // At t = 1.0, a DidChangeAnimated notification should be sent
            // from finish().
            if t < 1.0 {
                if let Some(frame_fn) = &frame_animation.transition_frame_fn {
                    frame_fn(t as f64);
                }
                transform.observer.on_camera_is_changing();
                false
            } else {
                // Indicate that we need to terminate this transition
                true
            }
        }));

        let finish_animation = animation.clone();
        self.transition_finish_fn = Some(Box::new(move |transform: &mut Transform| {
            transform.state.set_properties(
                &TransformStateProperties::default()
                    .with_panning_in_progress(Some(false))
                    .with_scaling_in_progress(Some(false))
                    .with_rotating_in_progress(Some(false)),
            );
            if let Some(finish) = &finish_animation.transition_finish_fn {
                finish();
            }
            transform.observer.on_camera_did_change(if is_animated {
                CameraChangeMode::Animated
            } else {
                CameraChangeMode::Immediate
            });
        }));

        if !is_animated {
            let update = self.transition_frame_fn.take();
            let finish = self.transition_finish_fn.take();

            if let Some(mut update) = update {
                update(self, Clock::now());
            }
            if let Some(mut finish) = finish {
                finish(self);
            }
        }
    }

    pub fn in_transition(&self) -> bool {
        self.transition_frame_fn.is_some()
    }

    pub fn update_transitions(&mut self, now: TimePoint) {
        // Use a temporary function to ensure that the transitionFrameFn
        // lambda is called only once per update.
        //
        // This addresses the symptoms of
        // https://github.com/mapbox/mapbox-gl-native/issues/11180 where
        // setting a shape source to nil (or similar) in the
        // `onCameraIsChanging` observer function causes
        // `Map::Impl::onUpdate()` to be called which in turn calls this
        // function (before the current iteration has completed), leading to
        // an infinite loop. See
        // https://github.com/mapbox/mapbox-gl-native/issues/5833 for a
        // similar, related, issue.
        //
        // By temporarily nulling the `transitionFrameFn` (and then restoring
        // it after the temporary has been called) we stop this recursion.
        //
        // It's important to note that the scope of this change is stop the
        // above crashes. It doesn't address any potential deeper issue (for
        // example user error, how often and when transition callbacks are
        // called).

        let mut transition = self.transition_frame_fn.take();

        let done = match transition.as_mut() {
            Some(t) => t(self, now),
            None => false,
        };
        if done {
            // If the transition indicates that it is complete, then we should
            // call the finish lambda (going via a temporary as above)
            let finish = self.transition_finish_fn.take();

            self.transition_finish_fn = None;
            self.transition_frame_fn = None;

            if let Some(mut finish) = finish {
                finish(self);
            }
        } else if self.transition_frame_fn.is_none() {
            // We have to check `transitionFrameFn` is nil here, since a new
            // transition may have been triggered in a user callback (from the
            // transition call above)
            self.transition_frame_fn = transition;
        }
    }

    pub fn get_transition_start(&self) -> TimePoint {
        self.transition_start
    }

    pub fn get_transition_duration(&self) -> Duration {
        self.transition_duration
    }

    pub fn cancel_transitions(&mut self) {
        if let Some(mut finish) = self.transition_finish_fn.take() {
            finish(self);
        }

        self.transition_frame_fn = None;
        self.transition_finish_fn = None;
    }

    // MARK: - Gesture

    pub fn set_gesture_in_progress(&mut self, in_progress: bool) {
        self.state.set_gesture_in_progress(in_progress);
    }

    pub fn is_gesture_in_progress(&self) -> bool {
        self.state.is_gesture_in_progress()
    }

    // MARK: - Transform state

    pub fn get_state(&self) -> &TransformState {
        &self.state
    }

    pub fn is_rotating(&self) -> bool {
        self.state.is_rotating()
    }

    pub fn is_scaling(&self) -> bool {
        self.state.is_scaling()
    }

    pub fn is_panning(&self) -> bool {
        self.state.is_panning()
    }

    // MARK: Conversion and projection

    pub fn lat_lng_to_screen_coordinate(&self, lat_lng: &LatLng) -> ScreenCoordinate {
        let mut point = self.state.lat_lng_to_screen_coordinate(lat_lng);
        point.y = self.state.get_size().height as f64 - point.y;
        point
    }

    /// `screenCoordinateToLatLng(const ScreenCoordinate&, LatLng::WrapMode = LatLng::Wrapped)`.
    pub fn screen_coordinate_to_lat_lng(&self, point: &ScreenCoordinate, wrap_mode: WrapMode) -> LatLng {
        let mut flipped_point = *point;
        flipped_point.y = self.state.get_size().height as f64 - flipped_point.y;
        self.state.screen_coordinate_to_lat_lng(&flipped_point, wrap_mode)
    }

    pub fn get_free_camera_options(&self) -> FreeCameraOptions {
        self.state.get_free_camera_options()
    }

    pub fn set_free_camera_options(&mut self, options: &FreeCameraOptions) {
        self.cancel_transitions();
        self.state.set_free_camera_options(options);
    }
}

/// We don't want to show horizon: limit max pitch based on edge insets.
/// (`Transform::getMaxPitchForEdgeInsets`, called on the state inside the
/// frame function.)
fn get_max_pitch_for_edge_insets(state: &TransformState, insets: &EdgeInsets) -> f64 {
    // See TransformState::getCenterOffset.
    let center_offset_y = 0.5 * (insets.top - insets.bottom);
    if center_offset_y == 0.0 {
        return state.get_max_pitch();
    }

    let height = state.get_size().height;
    debug_assert!(height != 0);
    let field_of_view = state.get_field_of_view() as f64;
    // Half of fov is the field of view above perspective center.
    let tangent_of_fov_above_center_angle = (0.5 + center_offset_y / height as f64) * 2.0 * (field_of_view / 2.0).tan();
    let fov_above_center = tangent_of_fov_above_center_angle.atan();
    std_max(state.get_min_pitch(), state.get_max_pitch() + field_of_view / 2.0 - fov_above_center)
}

#[cfg(test)]
#[path = "transform_test.rs"]
mod transform_test;
