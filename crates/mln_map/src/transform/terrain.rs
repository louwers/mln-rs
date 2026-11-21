//! The terrain under the camera: the ground a camera at rest stands on, the
//! altitude a command eases onto the ground it ends on, the altitude a
//! gesture holds, the ground every motion ends on, the eye kept above the
//! ground while a command or a gesture moves the camera, the planes that
//! moves around an anchor solve on, and the conversions between screen
//! points and the ground drawn under them.
//!
//! Every rule applies only while the map hands the transform a ground
//! ([`TerrainGround`]); without one, each path is the flat map's. The
//! rules for a center the terrain clamps:
//!
//! - **At rest**, the center stands on the ground under it, unless that is
//!   still loading.
//! - **A command** that moves the center eases its altitude onto the ground
//!   at the center it ends on ([`EasedAltitude`]).
//! - **A gesture** owns the altitude from when its fingers go down: the
//!   running commands stop easing theirs, and the gesture holds the
//!   altitude the camera has then, lifted over ground that reaches above
//!   the eye or the near plane, through its momentum. A command that takes
//!   the center from the momentum eases the altitude again, and the gesture
//!   holds it from where that command leaves it.
//! - **Every motion ends on the ground**, before its last camera event, so
//!   that the rule at rest finds nothing left to do
//!   ([`Transform::come_to_rest`]). How depends on how the motion ends:
//!   - A command that completes, reaching its last frame still owning
//!     every field it took, ends at the center, zoom, bearing and pitch it
//!     asked for, and its last frame stands on the ground at that center:
//!     what is left between where its eased altitude headed and that
//!     ground is taken up in altitude alone. It is never recentred.
//!   - Anything else that brings the camera to rest leaves the center
//!     wherever the motion stopped, maybe off the ground: a gesture's end,
//!     a momentum's end, a command cancelled or replaced with nothing left
//!     moving, or one that gave up its center or its altitude on the way.
//!     A center off the ground then moves onto it along the view, with the
//!     eye where it is, where the bounds and the zoom limits take that
//!     camera as it is; where they don't, or the view meets no drawn
//!     ground, the rule at rest takes the center up or down instead.
//!
//! The terrain moves a camera at rest, as ground loads or changes under it,
//! the clamp turns on or the terrain goes, without camera events, as in GL
//! JS: the map's snapshots show those moves. Everything else that the
//! terrain does to the camera happens inside the command or gesture that
//! moves it, before the `CameraDidChange` that ends it, so the camera a
//! host reads on that event is the one at rest.

use std::sync::Arc;

use glam::{DVec2, DVec3};
use mln_map_types::{Elevation, View};
use mln_util::constants::TILE_SIZE_D;
use mln_util::error::Error;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
use mln_util::interpolate::interpolate;
use mln_util::projection::Projection;

use super::gesture::AnchorPlane;
use super::transition::{AlongFn, Transition};
use super::{Transform, flip_y};

/// How many times a command's camera is raised over the ground in a frame,
/// as in GL JS: each raise turns the near plane, which can bring another
/// part of it under the ground.
const MAX_EYE_RAISES: usize = 3;

/// How many times the end of a motion moves the center onto the ground, and
/// how far off the ground, in meters, a center at rest may stand without
/// it (GL JS's `recenterOnTerrain`).
const RECENTER_PASSES: usize = 3;
const RECENTER_TOLERANCE: f64 = 1e-4;

/// How near the view's center, in squared pixels, an anchor stands at it:
/// a move around it leaves the center where it is (GL JS's).
const NEAR_CENTER: f64 = 1e-2;

/// The share of the way up from the center to the eye that an anchor's
/// plane must stay under (GL JS's
/// `TERRAIN_ANCHOR_MAX_CAMERA_ALTITUDE_FRACTION`).
const MAX_ANCHOR_RISE: f64 = 0.9;

/// The terrain the camera stands on, which the map hands the transform
/// while its style's terrain draws on a Mercator view.
#[derive(Clone, Debug)]
pub struct TerrainGround {
    /// The ground the renderer last drew, or `None` before it drew any:
    /// the eye then stays above sea level, and the ground under the center
    /// counts as loading.
    pub elevation: Option<Arc<dyn Elevation>>,
    /// Whether the center stands on the ground: at rest, and at the end of
    /// every motion.
    pub clamped: bool,
}

/// The center altitude of a command that moves the center over terrain
/// that clamps it: a straight line from the altitude the command starts at
/// to the ground at the center it ends on (GL JS's
/// `_startElevationEasing`). Ground that loads or changes there while the
/// command runs turns the line about the altitude reached so far, so the
/// camera heads for the new ground without a jump. While the ground there
/// is unknown, the line heads for the altitude the command was given, or
/// else stays level.
pub(super) struct EasedAltitude {
    from: f64,
    target: f64,
    /// Where the command's center ends, wrapped.
    end: LatLng,
    /// The share of the line covered at a frame's eased progress.
    along: AlongFn,
}

impl EasedAltitude {
    /// A line from `from` to the ground at `end`, or to `given` while that
    /// is unknown.
    pub(super) fn new(from: f64, given: f64, ground: Option<f64>, end: LatLng, along: AlongFn) -> Self {
        EasedAltitude { from, target: ground.unwrap_or(given), end, along }
    }

    /// Where the command's center ends.
    pub(super) fn end(&self) -> LatLng {
        self.end
    }

    /// Moves where the command's center ends, as the ground there changes.
    pub(super) fn move_end(&mut self, end: LatLng) {
        self.end = end;
    }

    /// The altitude at the eased progress `k`, once the line heads for
    /// `ground`, the ground at the end now, where it is known. The last
    /// frame (`k` = 1) stands on that ground.
    pub(super) fn at(&mut self, ground: Option<f64>, k: f64) -> f64 {
        let k = (self.along)(k);
        if let Some(ground) = ground.filter(|&ground| ground != self.target) {
            if k < 1.0 {
                let reached = interpolate(&self.from, &self.target, k);
                let slope = (ground - reached) / (1.0 - k);
                self.from = reached - k * slope;
            }
            self.target = ground;
        }
        if k < 1.0 { interpolate(&self.from, &self.target, k) } else { self.target }
    }
}

impl Transform {
    /// Sets the terrain the camera stands on, or `None` without terrain,
    /// which turns off every rule of the camera over terrain.
    ///
    /// A rule that stops applying drops what it kept: once the terrain stops
    /// clamping the center, a gesture no longer holds an altitude and
    /// commands no longer ease theirs, and once the terrain goes, the view
    /// fits the flat map again, a clamped center goes to sea level, and then
    /// anchors hold their locations at sea level, under them in that camera.
    /// A rule that applies again starts from the camera as it is.
    pub fn set_terrain(&mut self, terrain: Option<TerrainGround>) {
        let clamped = self.clamped();
        let unclamps = clamped && !terrain.as_ref().is_some_and(|terrain| terrain.clamped);
        let goes = self.terrain.is_some() && terrain.is_none();
        self.terrain = terrain;
        if unclamps {
            self.gesture.hold = None;
            for transition in &mut self.transitions {
                transition.stop_easing_altitude();
            }
        }
        if goes {
            self.gesture.anchor_plane = None;
            self.leave_ground(clamped);
            let view = self.state.view();
            for transition in &mut self.transitions {
                transition.hold_anchor_at_sea_level(view);
            }
        }
    }

    /// Fits the view's near and far planes to the flat map again, once there
    /// is no ground, and puts a `clamped` center at sea level, keeping its
    /// location and zoom.
    fn leave_ground(&mut self, clamped: bool) {
        if self.view().terrain_min_elevation().is_some() {
            self.state.set_terrain_min_elevation(None);
            if clamped && self.view().center_altitude() != 0.0 {
                self.state.set_center_altitude(0.0);
            }
        }
    }

    /// The terrain the camera stands on ([`Self::set_terrain`]).
    pub(crate) fn terrain(&self) -> Option<&TerrainGround> {
        self.terrain.as_ref()
    }

    /// The ground the renderer last drew, while there is terrain.
    pub(crate) fn ground(&self) -> Option<&Arc<dyn Elevation>> {
        self.terrain.as_ref()?.elevation.as_ref()
    }

    /// The ground at `location`, where the renderer drew it and it has
    /// loaded.
    pub(super) fn ground_at(&self, location: LatLng) -> Option<f64> {
        self.terrain.as_ref()?.elevation.as_ref()?.at(location)
    }

    /// Moves the camera onto the terrain's ground. While there is ground,
    /// the view's near and far planes fit the lowest ground of the tile
    /// under the center, and a camera at rest whose center is clamped
    /// stands on the ground there, unless it is still loading. A camera that
    /// a gesture or a command moves keeps the altitude they give it: a
    /// command that moves a clamped center eases it onto the ground it ends
    /// on, and a gesture holds it, lifted over newer ground that reaches
    /// above the eye or the near plane with the planes it draws with. Once
    /// there is no ground, the planes fit the flat map again, and a clamped
    /// center goes back to sea level, once.
    ///
    /// At rest it reads only the height and floor at the center
    /// ([`Elevation::at`] and [`Elevation::min_at`]), which are what the
    /// renderer compares to tell whether a frame's camera stood on its
    /// newest ground. The terrain moves the camera, not a command, so this
    /// reports no camera change. Returns whether the view changed.
    pub fn follow_terrain(&mut self) -> bool {
        let Some(TerrainGround { elevation, clamped }) = &self.terrain else { return false };
        let (elevation, clamped) = (elevation.clone(), *clamped);
        let ground = |view: &View| (view.terrain_min_elevation(), view.center_altitude());
        let before = ground(self.view());
        let center = self.view().center(WrapMode::Wrapped);
        // The planes first, so that a lift clears the near plane the frame
        // draws with.
        if let Some(elevation) = &elevation {
            let min_elevation = Some(elevation.min_at(center).unwrap_or(0.0));
            if min_elevation != before.0 {
                self.state.set_terrain_min_elevation(min_elevation);
            }
        } else {
            self.leave_ground(clamped);
        }
        // A command of the pitch or the altitude alone doesn't count as
        // moving the camera, but it holds the altitude too.
        let at_rest = !self.is_camera_moving() && !self.in_transition();
        if self.gesture_holds() {
            self.lift_hold();
        } else if let Some(elevation) = elevation.filter(|_| clamped && at_rest)
            && let Some(altitude) = elevation.at(center).filter(|&altitude| altitude != before.1)
        {
            self.state.set_center_altitude(altitude);
        }
        ground(self.view()) != before
    }

    /// Whether there is terrain and it clamps the center.
    pub(super) fn clamped(&self) -> bool {
        self.terrain.as_ref().is_some_and(|terrain| terrain.clamped)
    }

    /// Whether a gesture holds the altitude of a center that the terrain
    /// clamps: while it is in progress, but while a command that took the
    /// center from its momentum eases the altitude.
    fn gesture_holds(&self) -> bool {
        self.gesture_in_progress && self.clamped() && !self.transitions.iter().any(Transition::eases_altitude)
    }

    /// Keeps the eye out of the terrain while a gesture or a command moves
    /// the camera: a gesture over a clamped center lifts the altitude it
    /// holds ([`Self::lift_hold`]), and anything else is raised by pitch and
    /// zoom ([`Self::keep_eye_above`]). The view's near and far planes fit
    /// the ground under the center first, as at rest. Without terrain it
    /// does nothing.
    pub(super) fn keep_clear_of_ground(&mut self) {
        // The near plane, which the ground at the center fits, is the one the
        // frame draws with, also on the update that brings the first ground.
        if let Some(TerrainGround { elevation: Some(elevation), .. }) = &self.terrain {
            let min_elevation = Some(elevation.min_at(self.view().center(WrapMode::Wrapped)).unwrap_or(0.0));
            if min_elevation != self.view().terrain_min_elevation() {
                self.state.set_terrain_min_elevation(min_elevation);
            }
        }
        if self.gesture_holds() {
            self.lift_hold();
        } else {
            self.keep_eye_above();
        }
    }

    /// Puts a gesture's center, which the terrain clamps, at the altitude
    /// the gesture holds, lifted by as far as the ground reaches above the
    /// eye or the near plane at that altitude, so that the lift lowers again
    /// once the ground under them does. A lift moves the eye and the near
    /// plane straight up over the same ground, so one step clears them. The
    /// gesture holds the altitude it first lifts, which is where its fingers
    /// went down, or where the command that took the center from its
    /// momentum left it.
    fn lift_hold(&mut self) {
        let Some(terrain) = &self.terrain else { return };
        let elevation = terrain.elevation.clone();
        let held = *self.gesture.hold.get_or_insert(self.state.view().center_altitude());
        self.state.set_center_altitude(held);
        let shortfall = eye_shortfall(self.view(), elevation.as_deref());
        if shortfall > 0.0 {
            self.state.set_center_altitude(held + shortfall);
        }
    }

    /// Ends a motion over a clamped center on the ground, once nothing moves
    /// the camera any more: no gesture is in progress and no command runs.
    ///
    /// After a command that `completed`, the camera stays where it asked to
    /// be, and the ground under its center takes it up or down, as at rest.
    /// After anything else, a center that stands farther off the loaded
    /// ground under it than [`RECENTER_TOLERANCE`] moves along the view
    /// onto the ground its line of sight meets, with the eye where it is
    /// ([`TransformState::recenter_on`], GL JS's `recenterOnTerrain`), and
    /// then stands on the ground as at rest; where the line meets no drawn
    /// ground, or the limits refuse the move, the center stays and the
    /// ground under it takes it up or down.
    ///
    /// The caller reports the camera's change. Returns whether the view
    /// changed.
    pub(super) fn come_to_rest(&mut self, completed: bool) -> bool {
        let Some(TerrainGround { elevation: Some(elevation), clamped: true }) = &self.terrain else {
            return false;
        };
        if self.gesture_in_progress || !self.transitions.is_empty() {
            return false;
        }
        if completed {
            return self.follow_terrain();
        }
        let view = self.view();
        let ground = elevation.at(view.center(WrapMode::Wrapped));
        if !ground.is_some_and(|ground| (view.center_altitude() - ground).abs() > RECENTER_TOLERANCE) {
            return false;
        }
        let elevation = Arc::clone(elevation);
        let before = self.view().clone();
        let size = self.view().size();
        let center = flip_y(self.view(), &self.view().padding().center_in(size));
        // Each pass leaves the center off the ground by the change of the
        // scale of heights between the latitudes it moved across, which the
        // next pass, from the center's new latitude, all but removes.
        for _ in 0..RECENTER_PASSES {
            let Some(hit) = self.view().ground_under(&center, &*elevation) else { break };
            if !self.state.recenter_on(&hit) {
                break;
            }
            let view = self.view();
            let at = world_point(&view.center(WrapMode::Unwrapped));
            let off = elevation.surface_at(at).map_or(0.0, |surface| view.center_altitude() - surface);
            if off.abs() <= RECENTER_TOLERANCE {
                break;
            }
        }
        self.follow_terrain();
        *self.view() != before
    }

    /// The height of the plane on which a move around `anchor` (y down)
    /// holds the ground it grabbed under `grabbed`, the pointer before the
    /// move, over terrain (GL JS's `_terrainGestureElevation`): for a
    /// gesture and its momentum (`gesture`), the height of the ground the
    /// gesture grabbed at its first anchored step, which holds while the
    /// gesture holds the same altitude, so that the map moves as far for
    /// each pixel throughout; for a command, the height of the ground its own
    /// line of sight meets.
    ///
    /// `None`, for the plane at the center's altitude, without terrain,
    /// where the pointer grabbed no drawn ground, for a pivot (`grabbed` is
    /// `anchor`) at the view's center, and where the plane can't hold the
    /// anchor or the grabbed point ([`sees_plane`]).
    pub(super) fn anchor_plane(
        &mut self,
        grabbed: &ScreenCoordinate,
        anchor: &ScreenCoordinate,
        gesture: bool,
    ) -> Option<f64> {
        let elevation = self.terrain.as_ref()?.elevation.clone();
        let grab = |view: &View| {
            let ground = elevation.as_deref()?;
            Some(view.ground_under(&flip_y(view, grabbed), ground)?.height)
        };
        let height = if gesture {
            let altitude = self.gesture.hold.unwrap_or(self.view().center_altitude());
            match self.gesture.anchor_plane {
                Some(plane) if plane.at_altitude == altitude => plane.height,
                _ => {
                    let height = grab(self.view());
                    self.gesture.anchor_plane = Some(AnchorPlane { height, at_altitude: altitude });
                    height
                }
            }
        } else {
            grab(self.view())
        }?;
        let view = self.view();
        let center = view.padding().center_in(view.size());
        // A pivot at the center leaves the center where it is.
        let pivot = grabbed == anchor && (anchor.x - center.x).powi(2) + (anchor.y - center.y).powi(2) < NEAR_CENTER;
        let sees = |point: &ScreenCoordinate| sees_plane(view, &flip_y(view, point), height);
        (!pivot && sees(anchor) && sees(grabbed)).then_some(height)
    }

    /// The height of the plane on which a move around `anchor` holds the
    /// location under it ([`Self::anchor_plane`] for `grabbed`): the plane at
    /// the center's altitude where that is `None` over terrain, and sea
    /// level, the flat map, without terrain.
    pub(super) fn anchor_height(
        &mut self,
        grabbed: &ScreenCoordinate,
        anchor: &ScreenCoordinate,
        gesture: bool,
    ) -> f64 {
        if self.terrain.is_none() {
            return 0.0;
        }
        self.anchor_plane(grabbed, anchor, gesture).unwrap_or(self.view().center_altitude())
    }

    /// The center that moves the location on the plane `height` meters
    /// above sea level under `from` to `to`, both screen points (y down),
    /// unwrapped.
    pub(super) fn center_moving_on_plane(
        &self,
        from: &ScreenCoordinate,
        to: &ScreenCoordinate,
        height: f64,
    ) -> Result<LatLng, Error> {
        let location = self.screen_coordinate_to_lat_lng_at(from, height, WrapMode::Unwrapped)?;
        self.state.center_for_lat_lng_at(&location, &flip_y(self.view(), to), height)
    }

    /// Keeps the eye of a command's camera out of the terrain: where the
    /// eye, or the near plane over drawn ground, is below the ground, raises
    /// the eye by pitch and zoom, keeping its place and the center it looks
    /// at (GL JS's `_raiseCameraByPitchAndZoom`). Before the renderer drew
    /// any ground, the eye stays above sea level. Without terrain it does
    /// nothing.
    pub(super) fn keep_eye_above(&mut self) {
        let Some(terrain) = &self.terrain else { return };
        let elevation = terrain.elevation.clone();
        for _ in 0..MAX_EYE_RAISES {
            let shortfall = eye_shortfall(self.view(), elevation.as_deref());
            if !(shortfall > 0.0 && self.state.raise_eye(shortfall)) {
                return;
            }
        }
    }
}

/// Whether a move can hold a location on the plane `height` meters above
/// sea level under `point` of `view`, measured from the bottom edge (y up):
/// whether the point's line of sight descends and meets the plane in front
/// of the eye, before the far plane, where the view draws the ground, and
/// the plane stands less than nine tenths of the way up from the center to
/// the eye, where the line meets it at a depth deep enough to solve on. It
/// asks the line itself, which a roll turns away from the screen's rows.
pub(super) fn sees_plane(view: &View, point: &ScreenCoordinate, height: f64) -> bool {
    let Some((near, far)) = view.line_of_sight(point) else { return false };
    let altitude = view.center_altitude();
    let eye = view.eye_position().z;
    far.z < near.z && far.z <= height && height < eye && height - altitude < MAX_ANCHOR_RISE * (eye - altitude)
}

/// The location under `point` of `view`, measured from the bottom edge (y
/// up), over terrain: where its line of sight first meets `ground`, or the
/// plane at the center's altitude where it meets no drawn ground or before
/// any was drawn (GL JS's `screenPointToMercatorCoordinate` with terrain).
/// Fails as [`View::screen_coordinate_to_lat_lng`].
pub(crate) fn unproject_on_ground(
    view: &View,
    ground: Option<&dyn Elevation>,
    point: &ScreenCoordinate,
    wrap_mode: WrapMode,
) -> Result<LatLng, Error> {
    match ground.and_then(|ground| view.ground_under(point, ground)) {
        Some(hit) => Ok(hit.location(wrap_mode)),
        None => view.screen_coordinate_to_lat_lng_at(point, view.center_altitude(), wrap_mode),
    }
}

/// The screen point of `lat_lng` on `view`, measured from the bottom edge
/// (y up), standing on `ground`, or at sea level where none was drawn (GL
/// JS's `locationToScreenPoint` with terrain).
pub(crate) fn project_on_ground(view: &View, ground: Option<&dyn Elevation>, lat_lng: &LatLng) -> ScreenCoordinate {
    let height = ground.and_then(|ground| ground.surface_at(world_point(lat_lng))).unwrap_or(0.0);
    view.lat_lng_to_screen_coordinate_at(lat_lng, height)
}

/// Where `lat_lng` is in Web Mercator at zoom 0.
fn world_point(lat_lng: &LatLng) -> DVec2 {
    let world = Projection::project(lat_lng, 1.0 / TILE_SIZE_D);
    DVec2::new(world.x, world.y)
}

/// How far the ground reaches above the eye of `view`, or above a point of
/// its near plane over drawn ground, in meters, whichever is more; zero or
/// less while all are clear. The ground under the eye is sea level where
/// none was drawn, or before any was (`elevation` is `None`); then only the
/// eye counts.
fn eye_shortfall(view: &View, elevation: Option<&dyn Elevation>) -> f64 {
    let eye = view.eye_position();
    let above =
        |point: DVec3| elevation.and_then(|elevation| elevation.surface_at(point.truncate())).map(|h| h - point.z);
    let mut shortfall = above(eye).unwrap_or(-eye.z);
    if elevation.is_some() {
        for point in view.near_plane_points() {
            shortfall = above(point).map_or(shortfall, |above| shortfall.max(above));
        }
    }
    shortfall
}
