//! The terrain under the camera: the ground a camera at rest stands on, the
//! altitude a command eases toward the ground it ends on, the altitude a
//! gesture holds, the ground every motion ends on, the eye kept above the
//! ground while a command or a gesture moves the camera, the planes that
//! moves around an anchor solve on, and the conversions between screen
//! points and the ground drawn under them.
//!
//! Every rule applies only while the map hands the transform a ground
//! ([`TerrainGround`]); without one, the flat map's ground is the plane at
//! sea level, and the anchors hold the locations they grab on it. The rules
//! for a center the terrain clamps:
//!
//! - **At rest**, the center stands on the ground under it, unless that is
//!   still loading.
//! - **While commands of the center or the zoom run**, the altitude heads
//!   for the ground under the center the camera is expected to end on, to
//!   arrive with the center as the command of the center ends, or else as
//!   the last zoom ends ([`Transform::ease_altitude`]). Each frame derives
//!   that from the commands running then, so no command owns the line: one
//!   that is replaced, is cancelled or gives up its center, and ground that
//!   lands, leave the next frame to head on from where the altitude is.
//!   Commands of the angles alone hold it.
//! - **A gesture** holds the altitude from when its fingers go down: the
//!   altitude the camera has then, lifted over ground that reaches above
//!   the eye or the near plane, through its momentum. A command of the
//!   center or the zoom that runs once the fingers lift eases the altitude
//!   again, and the gesture holds it from where that leaves it.
//! - **Every motion ends on the ground**, before its last camera event, so
//!   that the rule at rest finds nothing left to do
//!   ([`Transform::come_to_rest`]). How depends on how the motion ends:
//!   - A command of the center or the zoom that runs to its end keeps the
//!     center and the zoom, and the ground under its center takes up what
//!     is left in altitude alone: what the eased altitude still lacked on
//!     its last frame.
//!   - Anything else that brings the camera to rest leaves the center
//!     wherever the motion stopped, maybe off the ground: a gesture's end,
//!     a momentum's end, a command cancelled or replaced with nothing left
//!     moving, or one of the angles alone, under which ground may have
//!     loaded. A center off the ground then moves onto it along the view,
//!     with the eye where it is, where the bounds and the zoom limits take
//!     that camera as it is; where they don't, or the view meets no drawn
//!     ground, the rule at rest takes the center up or down instead.
//!
//! The clamp decides the altitude, so a command or a gesture takes none it
//! is given, and one that sets only the altitude moves nothing; the
//! commands that run when the clamp turns on give theirs up. Where the
//! ground the altitude would head for is unknown, as before the renderer
//! drew any, the altitude stays where it is, and heads on from there once
//! that ground is known.
//!
//! The terrain moves a camera at rest, as ground loads or changes under it,
//! the clamp turns on or the terrain goes, without camera events, as in GL
//! JS: the map's snapshots show those moves. Everything else that the
//! terrain does to the camera happens inside the command or gesture that
//! moves it, before the `CameraDidChange` that ends it, so the camera a
//! host reads on that event is the one at rest.

use std::sync::Arc;

use glam::{DVec2, DVec3};
use mln_map_types::{CameraOptions, Elevation, View};
use mln_util::constants::TILE_SIZE_D;
use mln_util::error::Error;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
use mln_util::projection::Projection;
use mln_util::time::Instant;

use super::gesture::AnchorPlane;
use super::transition::Transition;
use super::{ALTITUDE, CENTER, Transform, flip_y};

/// How many times a command's camera is raised over the ground in a frame,
/// as in GL JS: each raise turns the near plane, which can bring another
/// part of it under the ground.
const MAX_EYE_RAISES: usize = 3;

/// How many times the end of a motion moves the center onto the ground (GL
/// JS's `recenterOnTerrain`).
const RECENTER_PASSES: usize = 3;

/// How far off the ground, in meters, a center may stand and count as on it
/// as a motion ends: a tenth of a millimeter, far below a pixel at any zoom.
const ON_GROUND_TOLERANCE: f64 = 1e-4;

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

impl Transform {
    /// Sets the terrain the camera stands on, or `None` without terrain,
    /// which turns off every rule of the camera over terrain.
    ///
    /// A rule that stops applying drops what it kept: once the terrain stops
    /// clamping the center, a gesture no longer holds an altitude; once the
    /// terrain goes, the view fits the flat map again, a clamped center goes
    /// to sea level, and then anchors hold their locations at sea level,
    /// under them in that camera. A rule that applies again starts from the
    /// camera as it is, and a clamp that does takes the running commands'
    /// altitudes.
    pub fn set_terrain(&mut self, terrain: Option<TerrainGround>) {
        let clamped = self.clamped();
        let clamps = terrain.as_ref().is_some_and(|terrain| terrain.clamped);
        let goes = self.terrain.is_some() && terrain.is_none();
        self.terrain = terrain;
        if clamped && !clamps {
            self.gesture.hold = None;
        }
        if clamps && !clamped {
            for transition in &mut self.transitions {
                transition.fields &= !ALTITUDE;
            }
        }
        if goes {
            self.gesture.anchor_plane = None;
            // Also before any ground was drawn, as the ground would have
            // overruled the altitude of a clamped center.
            if clamped && self.view().center_altitude() != 0.0 {
                self.state.set_center_altitude(0.0);
            }
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
        // A command of the pitch alone doesn't count as moving the camera,
        // but it holds the altitude too.
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
    /// clamps: while it is in progress, but while its fingers are up and a
    /// command eases the altitude.
    fn gesture_holds(&self) -> bool {
        self.gesture_in_progress && self.clamped() && !self.eases_altitude_now()
    }

    /// Whether the running commands ease the altitude of a center that the
    /// terrain clamps: while a command of the center or the zoom runs, and
    /// no gesture's fingers are down.
    fn eases_altitude_now(&self) -> bool {
        self.clamped() && !self.gesture.active && self.transitions.iter().any(Transition::eases_altitude)
    }

    /// Sets the altitude of `camera`, a frame's camera at `now` composed of
    /// the running commands, while they ease it ([`Self::eases_altitude_now`]):
    /// from the altitude the camera has, toward the ground under the center
    /// the camera is expected to end on ([`Transition::expected_end`]), by
    /// this frame's share of what is left of the way of the command of the
    /// center, or else of the zoom that ends last, so that it arrives as
    /// that command ends (GL JS's `_startElevationEasing`, turned toward its
    /// target each frame). Only the time of the frame before carries over
    /// from one frame to the next. While that ground is unknown, the
    /// altitude stays where it is.
    pub(super) fn ease_altitude(&mut self, camera: &mut CameraOptions, now: Option<Instant>) {
        let before = std::mem::replace(&mut self.last_frame, now);
        if !self.eases_altitude_now() {
            return;
        }
        // The gesture holds the altitude again from where this leaves it.
        self.gesture.hold = None;
        let Some(ground) = self.ground() else { return };
        let easing = || self.transitions.iter().filter(|transition| transition.eases_altitude());
        let pacing = easing()
            .find(|transition| transition.fields & CENTER != 0)
            .or_else(|| easing().max_by_key(|transition| transition.remaining(now)));
        let Some(pacing) = pacing else { return };
        let altitude = self.view().center_altitude();
        let end = pacing.expected_end(&self.state, camera, altitude, Some(&**ground));
        let Some(target) = ground.at(end) else { return };
        let share = pacing.share(now);
        let reached = before.map_or(0.0, |before| pacing.share(Some(before))).min(share);
        camera.center_altitude = Some(if share < 1.0 {
            altitude + (target - altitude) * (share - reached) / (1.0 - reached)
        } else {
            target
        });
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
    /// After a command that `keeps` the center and the zoom, having run to
    /// its end owning either, they stay, and the ground under the center
    /// takes it up or down, as at rest. After anything else, a center that
    /// stands farther off the loaded ground under it than
    /// [`ON_GROUND_TOLERANCE`] moves along the view onto the ground its
    /// line of sight meets, with the eye where it is
    /// ([`TransformState::recenter_on`], GL JS's `recenterOnTerrain`), and
    /// then stands on the ground as at rest; where the line meets no drawn
    /// ground, or the bounds or the zoom limits would move that camera
    /// (`recenter_on` refuses it), the center stays and the ground under it
    /// takes it up or down.
    ///
    /// The caller reports the camera's change. Returns whether the view
    /// changed.
    pub(super) fn come_to_rest(&mut self, keeps: bool) -> bool {
        let Some(TerrainGround { elevation: Some(elevation), clamped: true }) = &self.terrain else {
            return false;
        };
        if self.gesture_in_progress || !self.transitions.is_empty() {
            return false;
        }
        if keeps {
            return self.follow_terrain();
        }

        let view = self.view();
        let ground = elevation.at(view.center(WrapMode::Wrapped));
        if !ground.is_some_and(|ground| (view.center_altitude() - ground).abs() > ON_GROUND_TOLERANCE) {
            return false;
        }
        let elevation = Arc::clone(elevation);
        let before = self.view().clone();
        let size = self.view().size();
        let center = flip_y(self.view(), &self.view().padding().center_in(size));
        // Each pass leaves the center off the ground by the change of the
        // scale of heights between the latitudes it moved across, which the
        // next pass, from the center's new latitude, all but removes. A
        // pass that meets no ground or that the limits refuse leaves the
        // center where the motion did.
        let mut recentred = Some(self.state.clone());
        for _ in 0..RECENTER_PASSES {
            let Some(state) = recentred.as_mut() else { break };
            let hit = state.view().ground_under(&center, &*elevation);
            if !hit.is_some_and(|hit| state.recenter_on(&hit)) {
                recentred = None;
                break;
            }
            let view = state.view();
            let at = world_point(&view.center(WrapMode::Unwrapped));
            let off = elevation.surface_at(at).map_or(0.0, |surface| view.center_altitude() - surface);
            if off.abs() <= ON_GROUND_TOLERANCE {
                break;
            }
        }
        if let Some(state) = recentred {
            self.state = state;
        }
        self.follow_terrain();
        *self.view() != before
    }

    /// The height of the plane on which a move around `anchor` (y down)
    /// holds the ground it grabbed under `grabbed`, the pointer before the
    /// move (GL JS's `_terrainGestureElevation`): the flat map's ground is
    /// the plane at sea level; over terrain, for a gesture and its momentum
    /// (`gesture`), the height of the ground the gesture grabbed at its first
    /// anchored step, which holds while the gesture holds the same altitude,
    /// so that the map moves as far for each pixel throughout; for a
    /// command, the height of the ground its own line of sight meets.
    ///
    /// `None`, for the plane at the center's altitude, where the pointer
    /// grabbed no drawn ground, before the renderer drew any, for a pivot
    /// (`grabbed` is `anchor`) at the view's center, on the globe, and where
    /// the plane can't hold the anchor or the grabbed point ([`sees_plane`]).
    pub(super) fn anchor_plane(
        &mut self,
        grabbed: &ScreenCoordinate,
        anchor: &ScreenCoordinate,
        gesture: bool,
    ) -> Option<f64> {
        let terrain = self.terrain.as_ref().map(|terrain| terrain.elevation.clone());
        let grab = |view: &View| match &terrain {
            None => Some(0.0),
            Some(ground) => Some(view.ground_under(&flip_y(view, grabbed), ground.as_deref()?)?.height),
        };
        let height = if gesture && terrain.is_some() {
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
    /// location under it ([`Self::anchor_plane`] for `grabbed`), or else
    /// the plane at the center's altitude.
    pub(super) fn anchor_height(
        &mut self,
        grabbed: &ScreenCoordinate,
        anchor: &ScreenCoordinate,
        gesture: bool,
    ) -> f64 {
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
