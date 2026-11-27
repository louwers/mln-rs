//! The map's camera: the commands, animations, gestures and momentum that
//! move it within its limits.
//!
//! Each command is a transition whose frame function takes the state and
//! returns the camera at the command's progress, which `apply_transitions`
//! composes with the other commands' cameras. The transform calls no
//! observer: it queues what happens as `TransformEvent`s, which its owner
//! drains once the call returns, so no callback runs while the transform
//! changes. Over terrain, the rules of `terrain` stand the camera on the
//! ground and keep its eye above it.

use std::f64::consts::PI;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use geometry::Point;
use mln_style::error::Error;
use mln_style::geo::{LatLng, WrapMode};
use mln_style::size::Size;
use mln_style::time::Instant;
use mln_style::types::ProjectionDefinition;
use parking_lot::Mutex;

use crate::camera::{
    AnimationOptions, CameraLimits, CameraOptions, CenterAndBearing, ConstrainMode, FreeCameraOptions,
    NorthOrientation, ProjectionMode, TransformState, TransitionEnd, VerticalPerspectiveProjection, View, ViewportMode,
};
use crate::geo::constants::M2PI;
use crate::geo::projection::Projection;
use crate::geo::{EdgeInsets, ScreenCoordinate};
use crate::log_target;

mod anchor;
mod animation;
mod gesture;
mod momentum;
mod terrain;
mod transition;

use gesture::GestureState;
pub(crate) use terrain::{TerrainGround, lowest_ground};
use transition::{FrameFn, JointEnd, Transition};

use crate::map::events::{CameraChange, CameraChangeSource};

/// A frame time older than this is stale, and counts as unset (see
/// `Transform::set_frame_time`): longer than a frame at 10 fps, so any host
/// that draws sets a fresh one first.
pub(crate) const STALE_FRAME_TIME: Duration = Duration::from_millis(100);

/// Converts the given angle (in radians) to be numerically close to the
/// anchor angle, allowing it to be interpolated properly without sudden
/// jumps.
fn normalize_angle(mut angle: f64, anchor_angle: f64) -> f64 {
    if angle.is_nan() || anchor_angle.is_nan() {
        return 0.0;
    }

    angle = mln_style::math::wrap(angle, -PI, PI);
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

/// A `LatLng` from coordinates the transform state keeps valid, saturating
/// any that aren't (see [`LatLng::saturating`]).
fn lat_lng(lat: f64, lon: f64) -> LatLng {
    LatLng::saturating(lat, lon, WrapMode::Unwrapped)
}

/// An animation frame's insets, or `None` when one is NaN (infinite insets
/// interpolate to NaN): the frame keeps the insets it has.
fn frame_edge_insets(top: f64, left: f64, bottom: f64, right: f64) -> Option<EdgeInsets> {
    if [top, left, bottom, right].iter().any(|value| value.is_nan()) {
        log::error!(target: log_target::GENERAL, "an animation frame's insets are NaN");
        return None;
    }
    Some(EdgeInsets::new(top, left, bottom, right))
}

/// An animation frame's `Projection::unproject`, or `None` (logged) for a
/// point with no `LatLng`, such as NaN: the frame keeps the center and zoom,
/// as [`frame_edge_insets`] does.
fn frame_unproject(p: &Point<f64>, scale: f64, wrap_mode: WrapMode) -> Option<LatLng> {
    Projection::unproject(p, scale, wrap_mode)
        .map_err(|e| log::error!(target: log_target::GENERAL, "{}", e.message))
        .ok()
}

/// Identifies a camera command that the transform started, in the
/// `TransformEvent::CommandEnded` that reports its end. Each command the
/// transform starts gets its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CameraCommandId(u64);

/// How a camera command ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CommandEnd {
    pub command: CameraCommandId,
    /// The command's [`AnimationOptions::transition_id`].
    pub(crate) transition_id: Option<u64>,
    pub end: TransitionEnd,
}

/// What the transform reports, in order, for its owner to drain with
/// [`Transform::drain_events`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TransformEvent {
    CameraWillChange(CameraChange),
    CameraIsChanging,
    CameraDidChange(CameraChange),
    /// A command ended: every command ends exactly once, unless the
    /// transform is dropped with the command still running (see
    /// [`Transform::close`]).
    CommandEnded(CommandEnd),
}

pub(crate) type ProjectionForZoomFn = Rc<dyn Fn(f64) -> ProjectionDefinition>;

// The camera fields that a command sets and owns, as bits.
const CENTER: u16 = 1 << 0;
const ZOOM: u16 = 1 << 1;
const BEARING: u16 = 1 << 2;
const PITCH: u16 = 1 << 3;
const PADDING: u16 = 1 << 4;
const ALTITUDE: u16 = 1 << 5;
const ROLL: u16 = 1 << 6;
const FOV: u16 = 1 << 7;

/// The fields a command sets.
fn camera_fields(camera: &CameraOptions) -> u16 {
    let field = |set: bool, field: u16| if set { field } else { 0 };
    field(camera.center.is_some(), CENTER)
        | field(camera.zoom.is_some(), ZOOM)
        | field(camera.bearing.is_some(), BEARING)
        | field(camera.pitch.is_some(), PITCH)
        | field(camera.padding.is_some(), PADDING)
        | field(camera.center_altitude.is_some(), ALTITUDE)
        | field(camera.roll.is_some(), ROLL)
        | field(camera.fov.is_some(), FOV)
}

/// The time of the latest frame that a map's host drew, which the map's
/// animations sample at and its camera commands start from (see
/// `Transform::set_frame_time`). A clone shares the clock, so the thread
/// that draws can set it while the map lives on another.
#[derive(Clone, Debug, Default)]
pub struct FrameClock(Arc<Mutex<Option<Instant>>>);

impl FrameClock {
    pub fn set(&self, time: Option<Instant>) {
        *self.0.lock() = time;
    }

    #[must_use]
    pub fn get(&self) -> Option<Instant> {
        *self.0.lock()
    }
}

pub struct Transform {
    state: TransformState,

    transitions: Vec<Transition>,
    /// What the transform reports, for [`Self::drain_events`].
    events: Vec<TransformEvent>,
    /// The command being started, which the transitions it starts report
    /// their end to (see [`Self::command`]), unless `silent`.
    command: Option<CameraCommandId>,
    /// Set while a command starts transitions whose ends another of its
    /// transitions reports: a delta's changes before its momentum.
    silent: bool,
    next_command: u64,
    /// The commands made of several transitions, whose end waits for the
    /// last of them.
    joint_ends: Vec<JointEnd>,
    /// What the commands being started come from: a command unless a gesture
    /// or a momentum starts them (see [`Self::with_source`]).
    source: CameraChangeSource,
    gesture: GestureState,
    /// From a gesture's first change until it ends, its momentum included.
    /// Over terrain, a gesture holds the altitude, and comes to rest on the
    /// ground as it ends (see `terrain`).
    gesture_in_progress: bool,
    /// The fields that the running commands move (`CENTER`, `ZOOM`,
    /// `BEARING`), as of the last frame.
    moving: u16,
    /// The host's current frame time (see [`Self::set_frame_time`]).
    frame_time: FrameClock,
    projection_for_zoom: Option<ProjectionForZoomFn>,
    globe_drag_hold: Option<GlobeDragHold>,
    /// The terrain the camera stands on ([`Self::set_terrain`]).
    terrain: Option<TerrainGround>,
    /// The time of the last frame that composed the commands' cameras,
    /// from which the altitude over terrain heads on
    /// ([`Self::ease_altitude`]).
    last_frame: Option<Instant>,
}

/// The location a drag on the globe holds under the cursor, from the step
/// that grabbed it. The next step carries the drag on if it starts where
/// that one ended, at `cursor` with the camera and the view as that step
/// left them; any other step grabs the location under where it starts.
#[derive(Clone, Copy, Debug)]
struct GlobeDragHold {
    location: LatLng,
    /// The height in meters of the sphere `location` is on.
    height: f64,
    cursor: ScreenCoordinate,
    camera: CameraOptions,
    size: Size,
}

impl Transform {
    #[must_use]
    pub fn new(constrain_mode: ConstrainMode, viewport_mode: ViewportMode) -> Self {
        Self::from_state(TransformState::new(constrain_mode, viewport_mode))
    }

    pub(crate) fn from_state(state: TransformState) -> Self {
        Transform {
            state,
            transitions: Vec::new(),
            events: Vec::new(),
            command: None,
            silent: false,
            next_command: 0,
            joint_ends: Vec::new(),
            source: CameraChangeSource::Command,
            gesture: GestureState::default(),
            gesture_in_progress: false,
            moving: 0,
            frame_time: FrameClock::default(),
            projection_for_zoom: None,
            globe_drag_hold: None,
            terrain: None,
            last_frame: None,
        }
    }

    /// Drains what the transform reported since the last call, oldest
    /// first. The transform keeps its buffer for what it reports next.
    pub(crate) fn drain_events(&mut self) -> std::vec::Drain<'_, TransformEvent> {
        self.events.drain(..)
    }

    fn emit(&mut self, event: TransformEvent) {
        self.events.push(event);
    }

    // MARK: - Map View

    pub fn resize(&mut self, size: Size) -> Result<(), Error> {
        if size.is_empty() {
            return Err(Error::invalid_argument("failed to resize: size is empty"));
        }

        if self.state.view().size() == size {
            return Ok(());
        }

        self.emit(TransformEvent::CameraWillChange(CameraChange::default()));
        self.state.set_size(size);
        let (scale, center) = self.state.constrain_screen(self.state.view().scale());
        if let Some(center) = center {
            // Turns out that if you resize during a transition any changes
            // made to the state will be ignored :( So if we have to constrain
            // because of a resize and a transition is in progress - cancel
            // the transition!
            if self.in_transition() {
                self.cancel_transitions();
            }

            // Setting x and y isn't enough to change the center: Cc and Bc
            // must change too, which setLatLngZoom does.
            let zoom = crate::camera::scale_zoom(scale);
            self.state.set_lat_lng_zoom(&center, zoom);
            self.emit(TransformEvent::CameraDidChange(CameraChange::default()));

            return Ok(());
        }
        self.state.constrain_position(scale);

        self.emit(TransformEvent::CameraDidChange(CameraChange::default()));
        Ok(())
    }

    // MARK: - Camera

    /// Returns the current camera options.
    #[must_use]
    pub(crate) fn camera_options(&self, padding: Option<EdgeInsets>) -> CameraOptions {
        self.state.camera_options(padding)
    }

    /// Change any combination of center, zoom, bearing, and pitch, without a
    /// transition. Omitted options keep their current animations.
    pub(crate) fn jump_to(&mut self, camera: &CameraOptions) -> CameraCommandId {
        self.command(|transform| transform.jump(camera))
    }

    fn jump(&mut self, camera: &CameraOptions) {
        // On the globe the zoom decides how close to a pole the center can be.
        // GL JS's jump bounds the center by the zoom it lands on, where its
        // ease bounds it by the zoom it starts from unless given one, so the
        // jump settles the zoom.
        if self.state.view().is_globe()
            && let Some(center) = camera.center
            && camera.zoom.is_none()
        {
            let zoom = self.view().zoom()
                + VerticalPerspectiveProjection::zoom_adjustment(
                    self.view().center(WrapMode::Wrapped).latitude(),
                    center.latitude(),
                );
            self.ease_to(&CameraOptions { zoom: Some(zoom), ..*camera }, &AnimationOptions::default());
            return;
        }
        self.ease_to(camera, &AnimationOptions::default());
    }

    // MARK: - Position

    /// Whether the globe is drawn on a sized screen, so screen points reach
    /// it. Before the first resize, screen-point gestures are Mercator's.
    fn globe_on_screen(&self) -> bool {
        self.state.view().is_globe() && !self.state.view().size().is_empty()
    }

    /// Pans the map by the given amount.
    /// Fails with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for a NaN
    /// or infinite offset.
    #[cfg(test)]
    pub(crate) fn move_by(
        &mut self,
        offset: &ScreenCoordinate,
        animation: &AnimationOptions,
    ) -> Result<CameraCommandId, Error> {
        self.move_by_with_anchor(offset, None, animation)
    }

    /// Moves the map by `offset`. A drag on the globe turns the planet so
    /// that the location under `anchor` follows the cursor. The ground under
    /// the cursor before the move follows it, on the plane at that ground's
    /// height, or on the globe the sphere: sea level on the flat map, the
    /// drawn ground over terrain ([`Self::set_terrain`]); without an anchor,
    /// or with one that grabs no ground, the map moves on the plane or the
    /// sphere at the center's altitude. On the globe, with the
    /// bearing held, steps that follow one another, each starting at the
    /// `anchor` of the one before with the camera as it left it, hold the
    /// location the first of them grabbed, wherever
    /// [`VerticalPerspectiveProjection::drag_center_holding`] has a center
    /// that does. Fails as [`move_by`](Self::move_by) does.
    pub(crate) fn move_by_with_anchor(
        &mut self,
        offset: &ScreenCoordinate,
        anchor: Option<ScreenCoordinate>,
        animation: &AnimationOptions,
    ) -> Result<CameraCommandId, Error> {
        let mut moved = Ok(());
        let command = self.command(|transform| moved = transform.move_by_now(offset, anchor, animation));
        moved.map(|()| command)
    }

    fn move_by_now(
        &mut self,
        offset: &ScreenCoordinate,
        anchor: Option<ScreenCoordinate>,
        animation: &AnimationOptions,
    ) -> Result<(), Error> {
        let flipped = |state: &TransformState, point: &ScreenCoordinate| {
            ScreenCoordinate::new(point.x, f64::from(state.view().size().height) - point.y)
        };
        let size = self.state.view().size();
        let view_center = self.state.view().padding().center_in(size);
        let fling_duration = animation.duration.filter(|duration| *duration > Duration::ZERO);
        let globe = self.globe_on_screen();
        if globe && (offset.x.is_nan() || offset.y.is_nan()) {
            return Err(Error::invalid_argument("offset must not be NaN"));
        }
        if globe && (!offset.x.is_finite() || !offset.y.is_finite()) {
            return Err(Error::invalid_argument("offset must not be infinite"));
        }
        if globe
            && let Some(duration) = fling_duration
            && self.state.view().globe_free_bearing()
        {
            // A fling with the bearing free turns the planet on about the axis
            // the drag turned it about, so it carries on over a pole instead
            // of spinning round it.
            let start = CenterAndBearing {
                center: self.view().center(WrapMode::Unwrapped),
                bearing: -mln_style::math::rad2deg(self.view().angle()),
            };
            let end = VerticalPerspectiveProjection::fling_camera(
                self.state.view(),
                &flipped(&self.state, &view_center),
                &ScreenCoordinate::new(offset.x, -offset.y),
            );
            let start_zoom = self.view().zoom();
            let frame: FrameFn = Box::new(move |state: &TransformState, t: f64| {
                let camera = VerticalPerspectiveProjection::interpolate_camera(&start, &end, t);
                let center = state.constrained_center(&camera.center, state.view().zoom());
                CameraOptions {
                    center: Some(center),
                    zoom: Some(
                        start_zoom
                            + VerticalPerspectiveProjection::zoom_adjustment(
                                start.center.latitude(),
                                center.latitude(),
                            ),
                    ),
                    bearing: Some(camera.bearing),
                    ..Default::default()
                }
            });
            // The fling sets the center, the zoom that keeps the planet's
            // size and the bearing.
            let fields = CameraOptions {
                center: Some(end.center),
                zoom: Some(start_zoom),
                bearing: Some(end.bearing),
                ..Default::default()
            };
            self.start_transition(&fields, animation, frame, None, duration, false);
            return Ok(());
        }

        if globe && fling_duration.is_some() {
            // An animated move, a fling, turns the globe as GL JS's inertia
            // does: the location under the far end of a long one can be off
            // the planet or over a pole. It stops short of half a turn, which
            // the ease would take the other way round.
            let mut center = self.state.constrained_center(
                &VerticalPerspectiveProjection::pan_center(self.state.view(), offset),
                self.view().zoom(),
            );
            let longitude = self.view().center(WrapMode::Unwrapped).longitude();
            if (center.longitude() - longitude).abs() > 180.0 {
                center = lat_lng(center.latitude(), longitude + 179.5f64.copysign(center.longitude() - longitude));
            }
            self.ease_to(&CameraOptions { center: Some(center), ..Default::default() }, animation);
            return Ok(());
        }

        if globe {
            // A drag turns the globe as GL JS's does: one rotation that brings
            // the location under the cursor back under it with the bearing
            // held, around the cursor when it is on the planet and around the
            // center otherwise. With a free bearing the rotation turns the
            // bearing too, so the location stays under the cursor.
            //
            // With the bearing held, that rotation loses the turn it would
            // give the bearing, so step after step the location slips from
            // under the cursor. Where a center holds it there exactly, the
            // drag takes that center instead, for the location its first step
            // grabbed. A step the exact center isn't for doesn't hold: the
            // step after it grabs anew, so the drag goes from one way to the
            // other without a jump.
            //
            // The location is on the sphere at the height of the ground the
            // drag grabbed, or else at the center's altitude, as the planes
            // of a drag on Mercator are.
            let from = anchor.map(|anchor| ScreenCoordinate::new(anchor.x - offset.x, anchor.y - offset.y));
            let held = self.globe_drag_hold.take().filter(|held| {
                from.is_some_and(|from| (held.cursor.x - from.x).hypot(held.cursor.y - from.y) < 1e-3)
                    && !self.state.view().globe_free_bearing()
                    && held.size == size
                    && held.camera == self.camera_options(None)
            });
            let height = match (held, anchor.zip(from)) {
                (Some(held), _) => held.height,
                (None, Some((anchor, from))) => self.anchor_height(&from, &anchor),
                (None, None) => self.view().center_altitude(),
            };
            let on_globe = |state: &TransformState, point: &ScreenCoordinate| {
                VerticalPerspectiveProjection::screen_coordinate_hits_globe(
                    state.view(),
                    &flipped(state, point),
                    height,
                )
            };
            let grab = match anchor {
                Some(anchor) if on_globe(&self.state, &anchor) => anchor,
                _ => view_center,
            };
            let pan_delta = ScreenCoordinate::new(offset.x, -offset.y);
            let mut camera = CameraOptions::default();
            let hold = match anchor.zip(from) {
                Some((anchor, from)) if !self.state.view().globe_free_bearing() && on_globe(&self.state, &anchor) => {
                    let location = match held {
                        Some(held) => Some(held.location),
                        None if on_globe(&self.state, &from) => self
                            .state
                            .view()
                            .screen_coordinate_to_lat_lng_at(&flipped(&self.state, &from), height, WrapMode::Unwrapped)
                            .ok(),
                        None => None,
                    };
                    location.and_then(|location| {
                        VerticalPerspectiveProjection::drag_center_holding(
                            self.state.view(),
                            &location,
                            &flipped(&self.state, &anchor),
                            height,
                        )
                        .map(|center| (location, anchor, center))
                    })
                }
                _ => None,
            };
            let mut center = if let Some((_, _, center)) = hold {
                center
            } else if self.state.view().globe_free_bearing() {
                let drag = VerticalPerspectiveProjection::drag_camera(
                    self.state.view(),
                    &flipped(&self.state, &grab),
                    &pan_delta,
                    height,
                );
                camera = CameraOptions { bearing: Some(drag.bearing), ..camera };
                drag.center
            } else {
                VerticalPerspectiveProjection::drag_center(
                    self.state.view(),
                    &flipped(&self.state, &grab),
                    &pan_delta,
                    height,
                )
            };
            center = self.state.constrained_center(&center, self.view().zoom());
            let zoom = self.view().zoom()
                + VerticalPerspectiveProjection::zoom_adjustment(
                    self.view().center(WrapMode::Wrapped).latitude(),
                    center.latitude(),
                );
            self.ease_to(&CameraOptions { center: Some(center), zoom: Some(zoom), ..camera }, animation);
            self.globe_drag_hold = hold.map(|(location, cursor, _)| GlobeDragHold {
                location,
                height,
                cursor,
                camera: self.camera_options(None),
                size,
            });
            return Ok(());
        }

        let mut center_offset = *offset;

        // Reduce the offset so that it never goes past the horizon. If it
        // goes past the horizon, the pan direction is opposite of the
        // intended direction.
        let pitch = self.state.view().pitch();
        let offset_length = offset.x.hypot(offset.y);
        if pitch > 0.0 && offset_length > 0.0 {
            let camera_to_center =
                0.5 * f64::from(self.state.view().size().height) / (self.state.view().fov() / 2.0).tan();
            let pixels_to_horizon = (camera_to_center / pitch.tan()).abs();
            // must be < 1 to keep the offset short of the horizon
            let horizon_factor = 0.75;
            let scale = pixels_to_horizon * horizon_factor / offset_length;
            if scale < 1.0 {
                center_offset = ScreenCoordinate::new(offset.x * scale, offset.y * scale);
            }
        }

        let size = self.state.view().size();
        let center = self.state.view().padding().center_in(size);
        let point_on_screen = ScreenCoordinate::new(center.x - center_offset.x, center.y - center_offset.y);
        // The ground the pointer grabbed follows it, on its plane; without an
        // anchor, the map moves on the plane at the center's altitude. Use
        // unwrapped LatLng to carry information about moveBy direction.
        let anchored = anchor.and_then(|anchor| {
            let grabbed = ScreenCoordinate::new(anchor.x - offset.x, anchor.y - offset.y);
            Some((grabbed, anchor, self.anchor_plane(&grabbed, &anchor)?))
        });
        let (from, to, height) = anchored.unwrap_or((point_on_screen, center, self.view().center_altitude()));
        let center = self.center_moving_on_plane(&from, &to, height)?;
        self.ease_to(&CameraOptions { center: Some(center), ..Default::default() }, animation);
        Ok(())
    }

    // MARK: - Limits

    /// Sets the limits the camera moves within (see
    /// [`TransformState::set_limits`]).
    pub(crate) fn set_limits(&mut self, limits: CameraLimits) -> Result<(), Error> {
        self.state.set_limits(limits)
    }

    pub(crate) fn set_frustum_offset(&mut self, frustum_offset: &EdgeInsets) {
        self.state.set_frustum_offset(frustum_offset);
    }

    // MARK: - Projection

    pub(crate) fn set_projection_definition(&mut self, definition: &ProjectionDefinition) {
        self.state.set_projection_definition(definition);
    }

    /// The projection the style gives at a zoom, which constrains an
    /// animation's target as it constrains a jump.
    pub(crate) fn set_projection_for_zoom(&mut self, projection_for_zoom: Option<ProjectionForZoomFn>) {
        self.projection_for_zoom = projection_for_zoom;
    }

    pub(crate) fn set_globe_free_bearing(&mut self, free_bearing: bool) {
        self.state.set_globe_free_bearing(free_bearing);
    }

    pub(crate) fn is_globe_free_bearing(&self) -> bool {
        self.state.view().globe_free_bearing()
    }

    #[cfg(test)]
    pub(crate) fn is_location_occluded(&self, lat_lng: &LatLng) -> bool {
        self.state.view().is_location_occluded(lat_lng)
    }

    // MARK: - North Orientation

    pub(crate) fn set_north_orientation(&mut self, orientation: NorthOrientation) {
        self.state.set_north_orientation(orientation);
        self.state.constrain_position(self.state.view().scale());
    }

    // MARK: - Constrain mode

    pub(crate) fn set_constrain_mode(&mut self, mode: ConstrainMode) {
        self.state.set_constrain_mode(mode);
        self.state.constrain_position(self.state.view().scale());
    }

    // MARK: - Viewport mode

    pub(crate) fn set_viewport_mode(&mut self, mode: ViewportMode) {
        self.state.set_viewport_mode(mode);
    }

    /// Sets whether the center stands on the terrain. The map hands the
    /// setting to [`set_terrain`](Self::set_terrain) on its next update.
    pub(crate) fn set_center_clamped_to_ground(&mut self, clamped: bool) {
        self.state.set_center_clamped_to_ground(clamped);
    }

    // MARK: - Projection mode

    pub(crate) fn set_projection_mode(&mut self, options: &ProjectionMode) {
        if let Some(x_skew) = options.x_skew {
            self.state.set_x_skew(x_skew);
        }
        if let Some(y_skew) = options.y_skew {
            self.state.set_y_skew(y_skew);
        }
        if let Some(axonometric) = options.axonometric {
            self.state.set_axonometric(axonometric.as_bool());
        }
    }

    // MARK: - Frame clock

    /// Puts the camera's animations on the host's frame clock: commands start
    /// at the latest frame's time rather than at `Instant::now()` when they are
    /// made, and the map samples them at it. Each frame then shows its
    /// animations exactly one frame further on, whenever the input or the
    /// map's update ran, and an animation started between frames moves by
    /// that frame's interval on the next one: no frame repeats the
    /// camera, and none skips ahead. `None` goes back to `Instant::now()`.
    ///
    /// Orbis starts animations on the first frame after the call, which
    /// keeps the camera still for that frame; a glide that continues a drag
    /// would pause there.
    pub(crate) fn set_frame_time(&mut self, time: Option<Instant>) {
        self.frame_time.set(time);
    }

    /// The clock that [`set_frame_time`](Self::set_frame_time) sets.
    pub(crate) fn frame_clock(&self) -> FrameClock {
        self.frame_time.clone()
    }

    /// The host's frame time, unless it is stale: older than
    /// [`STALE_FRAME_TIME`], as when the host stopped drawing frames or
    /// forgot to set it this frame. A stale frame time would start a command
    /// in the past, which the next frame would show already under way or
    /// over.
    pub(crate) fn frame_time(&self) -> Option<Instant> {
        self.fresh_frame_time(Instant::now())
    }

    fn fresh_frame_time(&self, now: Instant) -> Option<Instant> {
        self.frame_time.get().filter(|&time| now.saturating_duration_since(time) <= STALE_FRAME_TIME)
    }

    /// When commands start: the frame time, or now.
    fn now(&self) -> Instant {
        let now = Instant::now();
        self.fresh_frame_time(now).unwrap_or(now)
    }

    /// Whether a gesture or a command moves the camera: the renderer draws
    /// such frames for speed rather than crispness.
    #[must_use]
    pub(crate) fn is_camera_moving(&self) -> bool {
        self.gesture_in_progress || self.moving & (CENTER | ZOOM | BEARING) != 0
    }

    // MARK: - Transform state

    #[must_use]
    pub(crate) fn state(&self) -> &TransformState {
        &self.state
    }

    /// The view the camera shows now.
    #[must_use]
    pub fn view(&self) -> &View {
        self.state.view()
    }

    #[cfg(test)]
    pub(crate) fn is_rotating(&self) -> bool {
        self.moving & BEARING != 0
    }

    #[cfg(test)]
    pub(crate) fn is_scaling(&self) -> bool {
        self.moving & ZOOM != 0
    }

    #[cfg(test)]
    pub(crate) fn is_panning(&self) -> bool {
        self.moving & CENTER != 0
    }

    // MARK: Conversion and projection

    #[must_use]
    pub(crate) fn lat_lng_to_screen_coordinate(&self, lat_lng: &LatLng) -> ScreenCoordinate {
        let mut point = self.state.view().lat_lng_to_screen_coordinate(lat_lng);
        point.y = f64::from(self.state.view().size().height) - point.y;
        point
    }

    /// The location under `point`, in screen coordinates (y down).
    /// Fails with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for a NaN
    /// or infinite point.
    pub(crate) fn screen_coordinate_to_lat_lng(
        &self,
        point: &ScreenCoordinate,
        wrap_mode: WrapMode,
    ) -> Result<LatLng, Error> {
        let mut flipped_point = *point;
        flipped_point.y = f64::from(self.state.view().size().height) - flipped_point.y;
        self.state.view().screen_coordinate_to_lat_lng(&flipped_point, wrap_mode)
    }

    /// Where the line of sight through `point` (y down) meets the plane
    /// `height` meters above sea level, on which the camera's moves solve
    /// (see `anchor`). Fails as
    /// [`View::screen_coordinate_to_lat_lng_at`].
    pub(crate) fn screen_coordinate_to_lat_lng_at(
        &self,
        point: &ScreenCoordinate,
        height: f64,
        wrap_mode: WrapMode,
    ) -> Result<LatLng, Error> {
        self.view().screen_coordinate_to_lat_lng_at(&flip_y(self.view(), point), height, wrap_mode)
    }

    pub(crate) fn set_free_camera_options(&mut self, options: &FreeCameraOptions) {
        self.cancel_transitions();
        self.state.set_free_camera_options(options);
    }
}

/// `point` measured from the other edge of `view`'s height: a screen point
/// (y down) as a view point (y up), or back.
pub(crate) fn flip_y(view: &View, point: &ScreenCoordinate) -> ScreenCoordinate {
    ScreenCoordinate::new(point.x, f64::from(view.size().height) - point.y)
}

/// `seconds` truncated to whole nanoseconds; `None`
/// when negative, NaN, or too long for a `Duration` made of nanoseconds. The
/// bound is exclusive because `u64::MAX` rounds up as a double.
fn nanos_from_secs(seconds: f64) -> Option<Duration> {
    (seconds >= 0.0 && seconds * 1e9 < u64::MAX as f64).then(|| Duration::from_nanos((seconds * 1e9) as u64))
}

/// The most pitch that `insets` allow without showing the horizon above the
/// padded area's center.
fn max_pitch_for_padding(state: &TransformState, insets: &EdgeInsets) -> f64 {
    // As `View::center_offset`.
    let center_offset_y = 0.5 * (insets.top - insets.bottom);
    if center_offset_y == 0.0 {
        return state.limits().max_pitch;
    }

    let height = state.view().size().height;
    debug_assert!(height != 0);
    let field_of_view = state.view().fov();
    // Half of fov is the field of view above perspective center.
    let tangent_of_fov_above_center_angle =
        (0.5 + center_offset_y / f64::from(height)) * 2.0 * (field_of_view / 2.0).tan();
    let fov_above_center = tangent_of_fov_above_center_angle.atan();
    f64::max(state.limits().min_pitch, state.limits().max_pitch + field_of_view / 2.0 - fov_above_center)
}

#[cfg(test)]
#[path = "transform_test.rs"]
mod transform_test;
