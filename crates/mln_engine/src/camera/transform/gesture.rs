//! Gestures: camera deltas and updates relative to where the camera is.

use super::{BEARING, CameraCommandId, PITCH, Transform, TransformEvent, ZOOM};
use crate::camera::{
    AnimationOptions, CameraDelta, CameraError, CameraMomentum, CameraOptions, CameraUpdate, CameraUpdateMode,
    GesturePhase, TransitionOutcome,
};
use crate::map::events::{CameraChange, CameraChangeMode, CameraChangeSource};

/// The gesture that camera deltas and updates with a phase make.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct GestureState {
    /// Between its first `Begin` or `Update` and its `End` or `Cancel`.
    pub(super) active: bool,
    /// Counts gestures, so that an earlier gesture's momentum doesn't end a
    /// later gesture.
    serial: u64,
    /// The fields the gesture changed (`ZOOM`, `BEARING`, `PITCH`), which
    /// alone may settle.
    changed: u16,
    /// The altitude the gesture holds over terrain, once it took it
    /// (see `terrain`).
    pub(super) altitude: Option<f64>,
}

impl GestureState {
    /// Records the fields a gesture's change moves.
    fn change(&mut self, zoom: bool, bearing: bool, pitch: bool) {
        self.changed |=
            (if zoom { ZOOM } else { 0 }) | (if bearing { BEARING } else { 0 }) | (if pitch { PITCH } else { 0 });
    }

    /// Drops the snaps of the parts the gesture didn't change: only what it
    /// changed settles.
    fn mask_snaps(&self, momentum: &mut CameraMomentum) {
        for (part, field) in
            [(&mut momentum.zoom, ZOOM), (&mut momentum.bearing, BEARING), (&mut momentum.pitch, PITCH)]
        {
            if self.changed & field == 0 {
                part.snap = None;
            }
        }
    }
}

fn gesture_source(phase: GesturePhase) -> CameraChangeSource {
    if phase == GesturePhase::None { CameraChangeSource::Command } else { CameraChangeSource::Gesture }
}

impl Transform {
    /// Moves the camera relative to where it is, as a gesture does
    /// (the C API's `mln_map_apply_camera_delta`). See
    /// [`CameraDelta`].
    pub fn apply_camera_delta(&mut self, delta: &CameraDelta) -> Result<CameraCommandId, CameraError> {
        delta.validate()?;
        Ok(self.command(|transform| transform.apply_camera_delta_now(delta)))
    }

    fn apply_camera_delta_now(&mut self, delta: &CameraDelta) {
        let source = gesture_source(delta.phase);
        let pans = delta.offset.is_some();
        let eases = delta.scale.is_some() || delta.bearing.is_some() || delta.pitch.is_some();
        // With momentum, the command's end is the momentum's, and the delta's
        // own changes, which can't animate, apply before it starts.
        // Otherwise the pan and the ease end the command together.
        let parts = match delta.momentum {
            Some(_) => AnimationOptions::default(),
            None => delta.animation,
        };
        self.silent = delta.momentum.is_some();
        self.join_ends(u32::from(pans) + u32::from(eases));
        self.gesture_phase_before(delta.phase);
        if delta.phase != GesturePhase::None {
            self.gesture.change(
                delta.scale.is_some_and(|scale| scale != 1.0),
                delta.bearing.is_some_and(|bearing| bearing != 0.0),
                delta.pitch.is_some_and(|pitch| pitch != 0.0),
            );
        }
        self.with_source(source, |transform| {
            if let Some(offset) = delta.offset {
                // Validation keeps the offset finite, so it has a LatLng. The
                // anchor is the point the drag holds, which the globe keeps
                // under it.
                let _ = transform.move_by_with_anchor(&offset, delta.anchor, &parts);
            }
            if eases {
                let current = transform.camera_options(None);
                let target = CameraOptions {
                    zoom: delta.scale.map(|scale| current.zoom.unwrap_or(0.0) + scale.log2()),
                    bearing: delta.bearing.map(|bearing| current.bearing.unwrap_or(0.0) + bearing),
                    pitch: delta.pitch.map(|pitch| current.pitch.unwrap_or(0.0) + pitch),
                    anchor: delta.anchor,
                    ..CameraOptions::default()
                };
                transform.ease_to(&target, &parts);
            }
        });
        self.silent = false;
        match delta.momentum {
            Some(mut momentum) => {
                self.gesture.mask_snaps(&mut momentum);
                // The gesture ends as its momentum starts, rather than in
                // `gesture_phase_after`, so that a momentum with nothing to
                // move, which ends at once, ends it too.
                self.gesture.active = false;
                let serial = self.gesture.serial;
                self.with_source(CameraChangeSource::Momentum, |transform| {
                    transform.start_momentum_now(&momentum, &delta.animation, Some(serial));
                });
            }
            None if !pans && !eases => self.end_unstarted(&delta.animation, TransitionOutcome::Completed),
            None => {}
        }
        self.gesture_phase_after(delta.phase, delta.momentum.is_some());
    }

    /// Moves the camera to absolute values, as a command or a gesture
    /// (the C API's `mln_map_update_camera`). See [`CameraUpdate`].
    pub fn update_camera(&mut self, update: &CameraUpdate) -> CameraCommandId {
        self.command(|transform| transform.update_camera_now(update))
    }

    fn update_camera_now(&mut self, update: &CameraUpdate) {
        self.gesture_phase_before(update.phase);
        if update.phase != GesturePhase::None {
            // A gesture's update can repeat a field it doesn't change.
            let current = self.camera_options(None);
            let changes = |value: Option<f64>, now: Option<f64>| value.is_some() && value != now;
            self.gesture.change(
                changes(update.camera.zoom, current.zoom),
                changes(update.camera.bearing, current.bearing),
                changes(update.camera.pitch, current.pitch),
            );
        }
        self.with_source(gesture_source(update.phase), |transform| match update.mode {
            CameraUpdateMode::Jump => {
                transform.ease(&update.camera, &AnimationOptions { duration: None, ..update.animation });
            }
            CameraUpdateMode::Ease => transform.ease(&update.camera, &update.animation),
            CameraUpdateMode::Fly => transform.fly(&update.camera, &update.animation, false),
        });
        self.gesture_phase_after(update.phase, false);
    }

    /// `Begin` cancels the running commands, and `Begin` and `Update` mark
    /// the gesture, before its change applies, so that the change follows
    /// the gesture rules (an ease doesn't fly, and wraps the longitude as a
    /// drag). The first phase after a gesture ended starts a new one, which
    /// an earlier gesture's momentum no longer ends.
    fn gesture_phase_before(&mut self, phase: GesturePhase) {
        if phase != GesturePhase::None && (phase == GesturePhase::Begin || !self.gesture.active) {
            self.gesture = GestureState { serial: self.gesture.serial.wrapping_add(1), ..GestureState::default() };
        }
        if phase == GesturePhase::Begin {
            self.cancel_transitions();
        }
        if matches!(phase, GesturePhase::Begin | GesturePhase::Update) {
            self.gesture.active = true;
            self.gesture_in_progress = true;
        }
    }

    /// `Cancel` cancels the running commands after its change applies, and
    /// `End` and `Cancel` end the gesture: at once, or when its momentum
    /// ends. A gesture that ends at once comes to rest
    /// ([`Self::come_to_rest`]), and reports the camera's change if that
    /// moves it, though the picture stays.
    fn gesture_phase_after(&mut self, phase: GesturePhase, momentum: bool) {
        if phase == GesturePhase::Cancel {
            self.cancel_transitions();
        }
        if matches!(phase, GesturePhase::End | GesturePhase::Cancel) {
            self.gesture.active = false;
            if !momentum {
                self.gesture_in_progress = false;
                if self.come_to_rest() {
                    let change =
                        CameraChange { mode: CameraChangeMode::Immediate, source: CameraChangeSource::Gesture };
                    self.emit(TransformEvent::CameraWillChange(change));
                    self.emit(TransformEvent::CameraDidChange(change));
                }
            }
        }
    }

    /// The end of a gesture's momentum ends the gesture, unless another
    /// gesture started since. The caller brings the camera to rest.
    pub(super) fn end_gesture_momentum(&mut self, gesture: Option<u64>) {
        if gesture == Some(self.gesture.serial) && !self.gesture.active {
            self.gesture_in_progress = false;
        }
    }

    #[cfg(test)]
    pub(crate) fn set_gesture_in_progress(&mut self, in_progress: bool) {
        self.gesture_in_progress = in_progress;
    }

    #[must_use]
    pub fn is_gesture_in_progress(&self) -> bool {
        self.gesture_in_progress
    }
}
