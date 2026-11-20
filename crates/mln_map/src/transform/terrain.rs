//! The terrain under the camera: the ground a camera at rest stands on, the
//! altitude a command eases onto the ground it ends on, and the eye kept
//! above the ground while a command moves the camera.

use std::sync::Arc;

use glam::DVec3;
use mln_map_types::{Elevation, View};
use mln_util::geo::{LatLng, WrapMode};
use mln_util::interpolate::interpolate;

use super::Transform;
use super::transition::AlongFn;

/// How many times a command's camera is raised over the ground in a frame,
/// as in GL JS: each raise turns the near plane, which can bring another
/// part of it under the ground.
const MAX_EYE_RAISES: usize = 3;

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
    /// on. Once there is no ground, the planes fit the flat map again, and a
    /// clamped center goes back to sea level, once.
    ///
    /// It reads only the height and floor at the center
    /// ([`Elevation::at`] and [`Elevation::min_at`]), which are what the
    /// renderer compares to tell whether a frame's camera stood on its
    /// newest ground. The terrain moves the camera, not a command, so this
    /// reports no camera change. Returns whether the view changed.
    pub fn follow_terrain(&mut self) -> bool {
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
        ground(self.view()) != before
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
