//! The terrain under the camera: the ground a camera at rest stands on, the
//! altitude a command eases onto the ground it ends on, the altitude a
//! gesture holds and the ground its end moves the center onto, the eye kept
//! above the ground while a command or a gesture moves the camera, the
//! planes that moves around an anchor solve on, and the conversions between
//! screen points and the ground drawn under them.
//!
//! Every rule applies only while the map hands the transform a ground
//! ([`TerrainGround`]); without one, each path is the flat map's. The
//! terrain moves a camera at rest, as ground loads or changes under it, the
//! clamp turns on or the terrain goes, without camera events, as in GL JS:
//! the map's snapshots show those moves. Everything else that the terrain
//! does to the camera happens inside the command or gesture that moves it,
//! before the `CameraDidChange` that ends it, so the camera a host reads on
//! that event is the one at rest.

use std::sync::Arc;

use glam::{DVec2, DVec3};
use mln_map_types::{Elevation, View};
use mln_util::constants::TILE_SIZE_D;
use mln_util::error::Error;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
use mln_util::interpolate::interpolate;
use mln_util::projection::Projection;

use super::gesture::AnchorPlane;
use super::transition::AlongFn;
use super::{CENTER, Transform, ZOOM, flip_y};

/// How many times a command's camera is raised over the ground in a frame,
/// as in GL JS: each raise turns the near plane, which can bring another
/// part of it under the ground.
const MAX_EYE_RAISES: usize = 3;

/// How many times a gesture's end moves the center onto the ground, and how
/// far off the ground, in meters, it may then stand (GL JS's
/// `recenterOnTerrain`).
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
    /// a command that moves it.
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

    /// Whether the line heads for `ground`.
    pub(super) fn heads_for(&self, ground: f64) -> bool {
        ground == self.target
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
    pub fn set_terrain(&mut self, terrain: Option<TerrainGround>) {
        if let Some(terrain) = &terrain {
            self.clamped_to_ground = terrain.clamped;
        }
        self.terrain = terrain;
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
    /// above the eye or the near plane. Once there is no ground, the planes
    /// fit the flat map again, and a clamped center goes back to sea level,
    /// once.
    ///
    /// At rest it reads only the height and floor at the center
    /// ([`Elevation::at`] and [`Elevation::min_at`]), which are what the
    /// renderer compares to tell whether a frame's camera stood on its
    /// newest ground. The terrain moves the camera, not a command, so this
    /// reports no camera change. Returns whether the view changed.
    pub fn follow_terrain(&mut self) -> bool {
        let before = self.view().center_altitude();
        let lifted = self.gesture_holds() && {
            self.lift_hold();
            self.view().center_altitude() != before
        };
        let (altitude, min_elevation) = match self.terrain.as_ref() {
            Some(TerrainGround { elevation: Some(elevation), clamped }) => {
                let center = self.view().center(WrapMode::Wrapped);
                // A command of the pitch or the altitude alone doesn't count
                // as moving the camera, but it holds the altitude too.
                let at_rest = !self.is_camera_moving() && !self.in_transition();
                let altitude = if *clamped && at_rest { elevation.at(center) } else { None };
                (altitude, Some(elevation.min_at(center).unwrap_or(0.0)))
            }
            _ if self.view().terrain_min_elevation().is_some() => (self.clamped_to_ground.then_some(0.0), None),
            _ => return false,
        };
        let ground = |view: &View| (view.terrain_min_elevation(), view.center_altitude());
        let before = ground(self.view());
        if min_elevation != before.0 {
            self.state.set_terrain_min_elevation(min_elevation);
        }
        if let Some(altitude) = altitude.filter(|&altitude| altitude != before.1) {
            self.state.set_center_altitude(altitude);
        }
        lifted || ground(self.view()) != before
    }

    /// Whether there is terrain and it clamps the center.
    pub(super) fn clamped(&self) -> bool {
        self.terrain.as_ref().is_some_and(|terrain| terrain.clamped)
    }

    /// Whether a gesture holds the altitude of a center that the terrain
    /// clamps: while it is in progress, unless a command took the center
    /// from its momentum.
    fn gesture_holds(&self) -> bool {
        self.gesture_in_progress && !self.gesture.ceded && self.clamped()
    }

    /// Keeps the eye out of the terrain while a gesture or a command moves
    /// the camera: a gesture over a clamped center lifts the altitude it
    /// holds ([`Self::lift_hold`]), and anything else is raised by pitch and
    /// zoom ([`Self::keep_eye_above`]). Without terrain it does nothing.
    pub(super) fn keep_clear_of_ground(&mut self) {
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
    /// gesture holds the altitude it first lifts, which is where it began:
    /// its beginning stopped the commands that moved the altitude.
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

    /// At the end of a gesture over a clamped center, moves the center along
    /// the view onto the ground its line of sight meets, with the eye where
    /// it is ([`TransformState::recenter_on`]), and then stands it on the
    /// ground as at rest. Where the line meets no drawn ground, or the
    /// limits refuse the move, the center stays and the ground under it
    /// takes it up or down. A command that moves the center or the zoom on
    /// from there, such as one the gesture's momentum gave way to, ends on
    /// the ground by its own rules instead. Returns whether the view
    /// changed.
    pub(super) fn recenter_on_ground(&mut self) -> bool {
        let Some(TerrainGround { elevation, clamped: true }) = &self.terrain else {
            return false;
        };
        if self.transitions.iter().any(|transition| transition.fields & (CENTER | ZOOM) != 0) {
            return false;
        }
        let elevation = elevation.clone();
        let before = self.view().clone();
        if let Some(elevation) = elevation {
            let size = self.view().size();
            let center = flip_y(self.view(), &self.view().padding().center_in(size));
            // Each pass leaves the center off the ground by the change of the
            // scale of heights between the latitudes it moved across, which
            // the next pass, from the center's new latitude, all but removes.
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
    /// where the pointer grabbed no drawn ground, for an anchor at the view's
    /// center or above the horizon, and for a plane nine tenths or more of
    /// the way up from the center to the eye, where the line of sight meets
    /// it at too shallow a depth to solve on.
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
        let near_center = (anchor.x - center.x).powi(2) + (anchor.y - center.y).powi(2) < NEAR_CENTER;
        let below_horizon = anchor.y > center.y - view.mercator_horizon();
        let altitude = view.center_altitude();
        let solvable = height - altitude < MAX_ANCHOR_RISE * (view.eye_position().z - altitude);
        (!near_center && below_horizon && solvable).then_some(height)
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
