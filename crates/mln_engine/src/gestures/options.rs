//! How gestures move each part of the camera, and the recognizer's rates and
//! thresholds.

use crate::camera::{InvalidMomentum, MomentumDecay, MomentumSnap};

/// Values every `spacing` from `offset`, in the part's unit, that report a
/// [`GestureEvent::Notch`](crate::gestures::GestureEvent::Notch) as a gesture moves the
/// part to or across one, for the host to play as haptic feedback. Bearing
/// notches lie within one turn from the offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Notch {
    /// Positive.
    pub spacing: f64,
    pub offset: f64,
    /// The host's name for the feedback, such as which haptic to play, which
    /// the notch's events carry.
    pub kind: u32,
}

impl Notch {
    /// North.
    pub(crate) const NORTH: Notch = Notch { spacing: 360.0, offset: 0.0, kind: 0 };
}

/// How gestures move the map's content.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanOptions {
    /// Whether gestures pan: drags, two fingers moving together, and deltas.
    pub enabled: bool,
    /// How a release glides; without it, the pan stops where the gesture
    /// leaves it.
    pub momentum: Option<MomentumDecay>,
}

/// How gestures move the zoom, the bearing or the pitch.
#[derive(Clone, Debug, PartialEq)]
pub struct AxisOptions {
    /// Whether gestures move the part.
    pub enabled: bool,
    /// How a release glides; without it, the part stops where the gesture
    /// leaves it.
    pub momentum: Option<MomentumDecay>,
    /// Where the part settles once its glide ends, after a gesture that
    /// changed it.
    pub snap: Option<MomentumSnap>,
    /// Where the part reports notches while the recognizer's gestures, their
    /// glides and their snaps move it. Several notches at one value report
    /// one event, with the kind of the first in the list.
    pub notches: Vec<Notch>,
}

/// One part of the camera that a gesture moves along a single value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    /// In zoom levels.
    Zoom,
    /// In degrees clockwise from north.
    Bearing,
    /// In degrees from looking straight down.
    Pitch,
}

/// Why [`GestureOptions`] are out of range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InvalidGestureOptions {
    #[error("the momentum of {0}: {1}")]
    Momentum(&'static str, InvalidMomentum),
    #[error("the snap of {0}: {1}")]
    Snap(&'static str, InvalidMomentum),
    #[error("a notch of {0} needs a positive spacing and a finite offset")]
    Notch(&'static str),
    #[error("a tap's zoom must be finite")]
    TapZoom,
}

/// How the recognizer moves each part of the camera. By default every part
/// moves and glides with its default decay ([`MomentumDecay::PAN`] and the
/// rest); the bearing settles on north within 7° and notches there; zoom and
/// pitch stay where gestures leave them, without notches
/// ([`MomentumSnap::ZOOM`] and [`MomentumSnap::PITCH`] settle them on a whole
/// level and flat); and a double tap zooms in a level and a two-finger tap
/// out.
///
/// Distances are in logical pixels, and times in milliseconds (`_ms`).
#[derive(Clone, Debug, PartialEq)]
pub struct GestureOptions {
    pub pan: PanOptions,
    pub zoom: AxisOptions,
    pub bearing: AxisOptions,
    pub pitch: AxisOptions,
    /// Zoom levels that a double tap zooms in, eased; the zoom-out mode zooms
    /// out instead. 0 leaves double taps to the host.
    pub double_tap_zoom: f64,
    /// Zoom levels that a two-finger tap zooms, eased; negative zooms out.
    /// 0 leaves two-finger taps to the host.
    pub two_finger_tap_zoom: f64,

    /// How far a press may move and still be a tap; drags start past it.
    pub(crate) click_tolerance: f64,
    /// How far apart two taps of a double tap may be, and the most time
    /// between them. A two-finger tap's fingers may move this far.
    pub(crate) tap_tolerance: f64,
    pub(crate) tap_interval_ms: f64,
    /// How long a press stays still to be a long press (Android's and iOS's
    /// 500 ms).
    pub(crate) long_press_ms: f64,
    /// Degrees of bearing per pixel of a sideways rotate drag: ffi's demo's
    /// 0.5 (GL JS, Compose and Orbis turn 0.8).
    pub(crate) drag_rotate: f64,
    /// Degrees of pitch per pixel of an upward rotate drag or two-finger
    /// drag.
    pub(crate) drag_pitch: f64,
    /// Zoom levels per pixel of a double-tap drag downward.
    pub(crate) tap_drag_zoom: f64,
    /// Zoom levels a pinch spreads or closes before it zooms.
    pub(crate) pinch_zoom_threshold: f64,
    /// Pixels along the fingers' circle a twist travels before it rotates.
    pub(crate) rotate_threshold: f64,
    /// How long a discrete delta eases (GL JS's wheel's 200 ms).
    pub(crate) step_duration_ms: f64,
    /// The most a delta zooms in one step, in levels: GL JS's
    /// `maxScalePerFrame` of 2. A discrete delta carries the rest to later
    /// frames.
    pub(crate) max_step_zoom: f64,
    /// Deltas are one gesture until this long after their last input and
    /// ease (Compose's and GL JS's 200 ms for scrolls).
    pub(crate) delta_idle_ms: f64,

    /// Whether a release that catches the recognizer's last glide (a flick
    /// that interrupts a glide going its way) adds that glide's speed, as
    /// `UIScrollView`, Android's scroll flywheel and Compose do. Each part's
    /// decay still caps the speed. No map app measured in
    /// maplibre-native-ffi#1705 does it (Apple Maps, Google Maps), nor do GL
    /// JS and ffi's demo.
    pub(crate) carry_caught_glide: bool,
    /// Whether a press stops every camera animation, as GL JS and the
    /// MapLibre Native SDKs do, or only the recognizer's own.
    pub(crate) interrupt_animations: bool,
    /// The [`AnimationOptions::transition_id`](crate::camera::AnimationOptions::transition_id)
    /// of the eases and momentum the recognizer starts.
    pub(crate) transition_id: u64,
}

impl GestureOptions {
    /// The options of `axis`.
    #[must_use]
    pub(crate) fn axis(&self, axis: Axis) -> &AxisOptions {
        match axis {
            Axis::Zoom => &self.zoom,
            Axis::Bearing => &self.bearing,
            Axis::Pitch => &self.pitch,
        }
    }

    /// Checks every part's momentum, snap and notches, and the taps' zooms.
    pub fn validate(&self) -> Result<(), InvalidGestureOptions> {
        if !self.double_tap_zoom.is_finite() || !self.two_finger_tap_zoom.is_finite() {
            return Err(InvalidGestureOptions::TapZoom);
        }
        let momentum = |name, momentum: Option<MomentumDecay>| {
            momentum
                .map_or(Ok(()), |decay| decay.validate().map_err(|error| InvalidGestureOptions::Momentum(name, error)))
        };
        momentum("pan", self.pan.momentum)?;
        for (name, part) in [("zoom", &self.zoom), ("bearing", &self.bearing), ("pitch", &self.pitch)] {
            momentum(name, part.momentum)?;
            if let Some(snap) = part.snap {
                snap.validate().map_err(|error| InvalidGestureOptions::Snap(name, error))?;
            }
            let valid = |notch: &Notch| notch.spacing.is_finite() && notch.spacing > 0.0 && notch.offset.is_finite();
            if !part.notches.iter().all(valid) {
                return Err(InvalidGestureOptions::Notch(name));
            }
        }
        Ok(())
    }
}

impl Default for GestureOptions {
    fn default() -> Self {
        let axis = |momentum: MomentumDecay, snap: Option<MomentumSnap>, notches: Vec<Notch>| AxisOptions {
            enabled: true,
            momentum: Some(momentum),
            snap,
            notches,
        };
        GestureOptions {
            pan: PanOptions { enabled: true, momentum: Some(MomentumDecay::PAN) },
            zoom: axis(MomentumDecay::ZOOM, None, Vec::new()),
            bearing: axis(MomentumDecay::BEARING, Some(MomentumSnap::BEARING), vec![Notch::NORTH]),
            pitch: axis(MomentumDecay::PITCH, None, Vec::new()),
            double_tap_zoom: 1.0,
            two_finger_tap_zoom: -1.0,
            click_tolerance: 3.0,
            tap_tolerance: 30.0,
            tap_interval_ms: 500.0,
            long_press_ms: 500.0,
            drag_rotate: 0.5,
            drag_pitch: 0.5,
            tap_drag_zoom: 1.0 / 128.0,
            pinch_zoom_threshold: 0.1,
            rotate_threshold: 25.0,
            step_duration_ms: 200.0,
            max_step_zoom: 1.0,
            delta_idle_ms: 200.0,
            carry_caught_glide: true,
            interrupt_animations: true,
            // "gestures" in ASCII.
            transition_id: 0x6765_7374_7572_6573,
        }
    }
}
