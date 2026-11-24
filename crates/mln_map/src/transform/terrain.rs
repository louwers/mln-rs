//! The terrain under the camera: the ground a camera at rest stands on, and
//! the eye kept above the ground while a command moves the camera.
//!
//! Every rule applies only while the map hands the transform a ground
//! ([`TerrainGround`]). The terrain moves a camera at rest, as ground loads
//! or changes under it, the clamp turns on or the terrain goes, without
//! camera events, as in GL JS: the map's snapshots show those moves.
//!
//! The rules are the same on the globe, where heights stand along each
//! point's vertical above the sea-level sphere: the same heights at the
//! center decide the altitude, and the eye, its near plane and the ground
//! under the line of sight are the globe camera's
//! ([`View::ground_under`]).

use std::sync::Arc;

use glam::DVec3;
use mln_map_types::{Elevation, View};
use mln_util::geo::WrapMode;

use super::Transform;

/// How many times a command's camera is raised over the ground in a frame,
/// as in GL JS: each raise turns the near plane, which can bring another
/// part of it under the ground.
const MAX_EYE_RAISES: usize = 3;

/// The terrain the camera stands on, which the map hands the transform
/// while its style's terrain draws outside tile mode, on every projection.
#[derive(Clone, Debug)]
pub struct TerrainGround {
    /// The ground the renderer last drew, or `None` before it drew any:
    /// the eye then stays above sea level, and the ground under the center
    /// counts as loading.
    pub elevation: Option<Arc<dyn Elevation>>,
    /// Whether the center stands on the ground at rest.
    pub clamped: bool,
}

impl Transform {
    /// Sets the terrain the camera stands on, or `None` without terrain,
    /// which turns off every rule of the camera over terrain. Once the
    /// terrain goes, the view fits the flat map again and a clamped center
    /// goes to sea level, keeping its location and zoom.
    pub fn set_terrain(&mut self, terrain: Option<TerrainGround>) {
        let clamped = self.clamped();
        let goes = self.terrain.is_some() && terrain.is_none();
        self.terrain = terrain;
        if goes {
            // Also before any ground was drawn, as the ground would have
            // overruled the altitude of a clamped center.
            if clamped && self.view().center_altitude() != 0.0 {
                self.state.set_center_altitude(0.0);
            }
            self.leave_ground(clamped);
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

    /// Moves the camera onto the terrain's ground. While there is ground,
    /// the view's near and far planes fit the lowest ground of the tile
    /// under the center, and a camera at rest whose center is clamped
    /// stands on the ground there, unless it is still loading. A camera that
    /// a gesture or a command moves keeps the altitude they give it. Once
    /// there is no ground, the planes fit the flat map again, and a clamped
    /// center goes back to sea level, once.
    ///
    /// It reads only the height and floor at the center ([`Elevation::at`]
    /// and [`Elevation::min_at`]), which are what the renderer compares to
    /// tell whether a frame's camera stood on its newest ground. The terrain
    /// moves the camera, not a command, so this reports no camera change.
    /// Returns whether the view changed.
    pub fn follow_terrain(&mut self) -> bool {
        let Some(TerrainGround { elevation, clamped }) = &self.terrain else { return false };
        let (elevation, clamped) = (elevation.clone(), *clamped);
        let ground = |view: &View| (view.terrain_min_elevation(), view.center_altitude());
        let before = ground(self.view());
        let center = self.view().center(WrapMode::Wrapped);
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
        if let Some(elevation) = elevation.filter(|_| clamped && at_rest)
            && let Some(altitude) = elevation.at(center).filter(|&altitude| altitude != before.1)
        {
            self.state.set_center_altitude(altitude);
        }
        ground(self.view()) != before
    }

    /// Whether there is terrain and it clamps the center.
    fn clamped(&self) -> bool {
        self.terrain.as_ref().is_some_and(|terrain| terrain.clamped)
    }

    /// Keeps the eye of a command's camera out of the terrain: the view's
    /// near and far planes fit the ground under the center, as at rest, and
    /// where the eye, or the near plane over drawn ground, is below the
    /// ground, the eye rises by pitch and zoom, keeping its place and the
    /// center it looks at (GL JS's `_raiseCameraByPitchAndZoom`). Before the
    /// renderer drew any ground, the eye stays above sea level. Without
    /// terrain it does nothing.
    pub(super) fn keep_eye_above(&mut self) {
        let Some(terrain) = &self.terrain else { return };
        let elevation = terrain.elevation.clone();
        if let Some(elevation) = &elevation {
            let min_elevation = Some(elevation.min_at(self.view().center(WrapMode::Wrapped)).unwrap_or(0.0));
            if min_elevation != self.view().terrain_min_elevation() {
                self.state.set_terrain_min_elevation(min_elevation);
            }
        }
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
