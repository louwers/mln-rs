//! The terrain under the camera. Every rule applies only while the map
//! hands the transform a ground ([`TerrainGround`]):
//!
//! - The eye stays above the ground while anything moves the camera
//!   ([`Transform::keep_clear_of_ground`]).
//! - A gesture holds the altitude, from its first step to the end of its
//!   momentum, lifted by as much as keeps the eye above the ground, which
//!   lowers again once the ground does. A command that runs while its
//!   fingers are up takes the altitude over, and the gesture holds it from
//!   where the command leaves it.
//! - Under the clamp, the center stands on the ground:
//!   - at rest, on the ground under it, where that is known;
//!   - while commands run and no fingers are down, the altitude eases
//!     toward the ground under where they leave the center, and stands on
//!     it as they end ([`Transform::ease_altitude`]);
//!   - brought to rest off the ground, by a cancel or by the end of a
//!     gesture or of its momentum, the center moves along the view onto the
//!     ground, with the eye where it is, so that the picture stays
//!     ([`Transform::come_to_rest`]).
//!
//! The rules are the same on the globe, where heights stand along each
//! point's vertical above the sea-level sphere: the same heights at the
//! center decide the altitude, and the eye, its near plane and the ground
//! under the line of sight are the globe camera's
//! ([`View::ground_under`]).
//!
//! The terrain moves a camera at rest, as ground loads or changes under it,
//! the clamp turns on or the terrain goes, without camera events, as in GL
//! JS: the map's snapshots show those moves. What the terrain does to a
//! command's or a gesture's camera happens within it, before the
//! `CameraDidChange` that ends it.

use std::sync::Arc;

use glam::DVec3;
use mln_style::geo::WrapMode;
use mln_style::time::Instant;

use super::transition::Transition;
use super::{CENTER, Transform, flip_y};
use crate::camera::{CameraOptions, Elevation, View};

/// How many times a moving camera is raised over the ground in a frame, as
/// in GL JS: each raise can turn or move the near plane, which can bring
/// another part of it under the ground.
const MAX_EYE_RAISES: usize = 3;

/// The terrain the camera stands on, which the map hands the transform
/// while its style's terrain draws outside tile mode, on every projection.
#[derive(Clone, Debug)]
pub(crate) struct TerrainGround {
    /// The ground the renderer last drew, or `None` before it drew any:
    /// the eye then stays above sea level, and the ground under the center
    /// counts as loading.
    pub(crate) elevation: Option<Arc<dyn Elevation>>,
    /// Whether the center stands on the ground at rest.
    pub(crate) clamped: bool,
}

impl Transform {
    /// Sets the terrain the camera stands on, or `None` without terrain,
    /// which turns off every rule of the camera over terrain. Once the
    /// terrain goes, the view fits the flat map again, a clamped center
    /// goes to sea level, keeping its location and zoom, and a gesture
    /// holds the altitude from where that leaves it.
    pub(crate) fn set_terrain(&mut self, terrain: Option<TerrainGround>) {
        let clamped = self.clamped();
        let goes = self.terrain.is_some() && terrain.is_none();
        self.terrain = terrain;
        if goes {
            self.gesture.altitude = None;
            self.leave_ground();
            // Also before any ground was drawn, as the ground would have
            // overruled the altitude of a clamped center.
            if clamped {
                self.go_to_sea_level();
            }
        }
    }

    /// Fits the view's near and far planes to the flat map again, and
    /// returns whether they fit the ground before.
    fn leave_ground(&mut self) -> bool {
        let fit = self.view().terrain_min_elevation().is_some();
        if fit {
            self.state.set_terrain_min_elevation(None);
        }
        fit
    }

    /// Puts the center at sea level, keeping its location and zoom.
    fn go_to_sea_level(&mut self) {
        if self.view().center_altitude() != 0.0 {
            self.state.set_center_altitude(0.0);
        }
    }

    /// Fits the view's near and far planes to the lowest ground that
    /// `elevation` has in the tile under the center, or to sea level where it
    /// has none.
    fn fit_planes(&mut self, elevation: &dyn Elevation) {
        let min_elevation = Some(lowest_ground(self.view(), elevation));
        if min_elevation != self.view().terrain_min_elevation() {
            self.state.set_terrain_min_elevation(min_elevation);
        }
    }

    /// Moves the camera onto the terrain's ground. While there is ground,
    /// the view's near and far planes fit the lowest ground of the tile
    /// under the center, and a camera at rest whose center is clamped
    /// stands on the ground there, unless it is still loading. A camera that
    /// a command moves takes the altitude its frames give it, which ease
    /// onto the ground, and a gesture that holds the altitude lifts it just
    /// over the ground as it is now, eye and near plane. Once there is no
    /// ground, the planes fit the flat map again, and a clamped center goes
    /// back to sea level, once.
    ///
    /// At rest it reads only the height and floor at the center
    /// ([`Elevation::at`] and [`Elevation::min_at`]), which are what the
    /// renderer compares to tell whether a frame's camera stood on its
    /// newest ground. The terrain moves the camera, not a command, so this
    /// reports no camera change. Returns whether the view changed.
    pub(crate) fn follow_terrain(&mut self) -> bool {
        let Some(TerrainGround { elevation, clamped }) = &self.terrain else { return false };
        let (elevation, clamped) = (elevation.clone(), *clamped);
        let ground = |view: &View| (view.terrain_min_elevation(), view.center_altitude());
        let before = ground(self.view());
        match &elevation {
            Some(elevation) => self.fit_planes(&**elevation),
            // Once, as the planes fit the flat map from then on.
            None => {
                if self.leave_ground() && clamped {
                    self.go_to_sea_level();
                }
            }
        }
        // A command of the pitch alone doesn't count as moving the camera,
        // but its frames set the altitude too.
        let at_rest = !self.is_camera_moving() && !self.in_transition();
        if self.gesture_holds_altitude() {
            self.lift(elevation.as_deref());
        } else if clamped && at_rest {
            self.stand_on_ground();
        }
        ground(self.view()) != before
    }

    /// Whether there is terrain and it clamps the center.
    pub(super) fn clamped(&self) -> bool {
        self.terrain.as_ref().is_some_and(|terrain| terrain.clamped)
    }

    /// Whether a gesture holds the altitude: from its first step to the end
    /// of its momentum, except while its fingers are up and a command runs.
    fn gesture_holds_altitude(&self) -> bool {
        self.gesture_in_progress && (self.gesture.active || !self.transitions.iter().any(Transition::is_command))
    }

    /// Puts the center at the height of the ground under it, where that is
    /// known.
    pub(super) fn stand_on_ground(&mut self) {
        let Some(TerrainGround { elevation: Some(ground), .. }) = &self.terrain else { return };
        let view = self.view();
        if let Some(altitude) = ground.at(view.center(WrapMode::Wrapped)).filter(|&h| h != view.center_altitude()) {
            self.state.set_center_altitude(altitude);
        }
    }

    /// Sets the altitude of `camera`, a frame's camera at `now` composed of
    /// the running commands, over terrain that clamps the center: from the
    /// altitude the camera has, toward the ground under where the commands
    /// leave the center, by this frame's share of what is left of the way of
    /// the command that ends last, so that it arrives as they end (GL JS's
    /// `_startElevationEasing`, turned toward its target each frame). Where
    /// they leave the center is where the command that owns it ends it, or
    /// else where it is. Ground that lands, or a command that starts, ends
    /// or hands the center over, turns the next frame toward the new target
    /// from where the altitude is. While that ground is unknown, as at the
    /// end of a flight past the ground the renderer drew, or a gesture holds
    /// the altitude, it stays where it is.
    ///
    /// Returns whether this frame ends the commands, which then stand the
    /// center on the ground under where they put it
    /// ([`Self::stand_on_ground`]).
    pub(super) fn ease_altitude(&mut self, camera: &mut CameraOptions, now: Option<Instant>) -> bool {
        let before = std::mem::replace(&mut self.last_frame, now);
        let Some(TerrainGround { elevation: Some(ground), clamped: true }) = &self.terrain else { return false };
        if self.gesture_holds_altitude() {
            return false;
        }
        let commands = || self.transitions.iter().filter(|transition| transition.is_command());
        let Some(pacing) = commands().max_by_key(|transition| transition.remaining(now)) else { return false };
        let end = match commands().find(|transition| transition.fields & CENTER != 0) {
            Some(owner) => owner.end_center(&self.state, camera, &**ground),
            None => self.view().center(WrapMode::Wrapped),
        };
        let Some(target) = ground.at(end) else { return false };
        let altitude = self.view().center_altitude();
        // An easing that overshoots its end, or backs up past its start,
        // takes the altitude no farther than the target: it arrives where
        // the share first reaches 1, and holds while the share falls back.
        let share = pacing.share(now).clamp(0.0, 1.0);
        let reached = before.map_or(0.0, |before| pacing.share(Some(before)).clamp(0.0, 1.0)).min(share);
        let lands = pacing.remaining(now).is_zero();
        camera.center_altitude = Some(if lands || reached == 1.0 {
            target
        } else {
            altitude + (target - altitude) * (share - reached) / (1.0 - reached)
        });
        lands
    }

    /// Stands a camera that a command, a gesture or a momentum brought to
    /// rest over terrain that clamps the center on the ground, once no
    /// gesture or command moves it. A center off the drawn ground under it,
    /// as a cancel or a gesture leaves it, moves along the view onto the
    /// ground its line of sight meets, with the eye where it is
    /// ([`TransformState::recenter_on`], GL JS's `recenterOnTerrain`); what
    /// that leaves off the ground, or all of it where the view meets no
    /// drawn ground or the limits refuse the move, the center takes up in
    /// altitude. The caller reports the change. Returns whether the camera
    /// moved.
    ///
    /// [`TransformState::recenter_on`]: crate::camera::TransformState::recenter_on
    pub(super) fn come_to_rest(&mut self) -> bool {
        let Some(TerrainGround { elevation: Some(ground), clamped: true }) = &self.terrain else { return false };
        let view = self.view();
        let off = ground.at(view.center(WrapMode::Wrapped)).is_some_and(|h| h != view.center_altitude());
        if !off || self.gesture_in_progress || !self.transitions.is_empty() {
            return false;
        }
        let ground = Arc::clone(ground);
        let center = flip_y(view, &view.padding().center_in(view.size()));
        if let Some(hit) = view.ground_under(&center, &*ground) {
            // A refused move leaves the state as it is.
            let _ = self.state.recenter_on(&hit);
        }
        self.stand_on_ground();
        true
    }

    /// Keeps the eye of a moving camera out of the terrain: the view's near
    /// and far planes fit the ground under the center, as at rest, and
    /// where the eye, or the near plane over drawn ground, would be below
    /// the ground, a gesture that holds the altitude lifts it
    /// ([`Self::lift`]), and anything else raises the eye by pitch and zoom,
    /// keeping its place and the center it looks at (GL JS's
    /// `_raiseCameraByPitchAndZoom`). Before the renderer drew any ground,
    /// the eye stays above sea level. Without terrain it does nothing.
    pub(super) fn keep_clear_of_ground(&mut self) {
        let Some(terrain) = &self.terrain else { return };
        let elevation = terrain.elevation.clone();
        // The frame's center may have moved since the planes last fit it.
        if let Some(elevation) = &elevation {
            self.fit_planes(&**elevation);
        }
        if self.gesture_holds_altitude() {
            self.lift(elevation.as_deref());
            return;
        }
        // The gesture holds the altitude again from where this leaves it.
        self.gesture.altitude = None;
        for _ in 0..MAX_EYE_RAISES {
            let shortfall = eye_shortfall(self.view(), elevation.as_deref());
            if !(shortfall > 0.0 && self.state.raise_eye(shortfall)) {
                return;
            }
        }
    }

    /// Puts the center at the altitude the gesture holds, raised by as far
    /// as the ground reaches above the eye or the near plane there, so that
    /// the lift lowers again once the ground under them does. The gesture
    /// holds the altitude the camera has when it first lifts it. On Mercator
    /// a raise moves the eye and the near plane straight up over the same
    /// ground, so one clears them; on the globe it takes up to as many as a
    /// command's.
    fn lift(&mut self, elevation: Option<&dyn Elevation>) {
        let mut altitude = *self.gesture.altitude.get_or_insert(self.view().center_altitude());
        for raise in 0..=MAX_EYE_RAISES {
            if self.view().center_altitude() != altitude {
                self.state.set_center_altitude(altitude);
            }
            let shortfall = eye_shortfall(self.view(), elevation);
            if !(shortfall > 0.0 && raise < MAX_EYE_RAISES) {
                return;
            }
            altitude += shortfall;
        }
    }
}

/// The lowest ground that `elevation` has in the tile under the center of
/// `view`, or sea level where it has none, in meters: what the view's near
/// and far planes fit while terrain draws.
pub(crate) fn lowest_ground(view: &View, elevation: &dyn Elevation) -> f64 {
    elevation.min_at(view.center(WrapMode::Wrapped)).unwrap_or(0.0)
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
