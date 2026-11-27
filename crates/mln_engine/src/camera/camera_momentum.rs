// The `PAN`, `ZOOM`, `BEARING` and `PITCH` constants are each part's default,
// so that every host and the gesture recognizer start from the same glide.

use std::time::Duration;

use crate::geo::ScreenCoordinate;

/// A unit cubic Bézier, from (0, 0) to (1, 1), with these control points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MomentumEasing {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

impl MomentumEasing {
    /// The curve that camera animations ease along by default.
    pub(crate) const DEFAULT: MomentumEasing = MomentumEasing { x1: 0.0, y1: 0.0, x2: 0.25, y2: 1.0 };
    /// The curve that Android's scroll fling follows.
    pub(crate) const ANDROID_FLING: MomentumEasing = MomentumEasing { x1: 0.175, y1: 0.5, x2: 0.35, y2: 1.0 };
}

impl Default for MomentumEasing {
    fn default() -> Self {
        MomentumEasing::DEFAULT
    }
}

/// How a glide unfolds from its release speed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MomentumDecayModel {
    /// The speed halves every `half_life`, until it drops to `stop_speed`.
    /// `UIScrollView`'s normal deceleration is a half-life of 346 ms, and its
    /// fast deceleration 69 ms.
    Friction { half_life: Duration, stop_speed: f64 },
    /// The glide lasts `duration` × (speed / `reference_speed`)^`exponent`
    /// and follows `easing`, starting at the release speed: it covers the
    /// speed × its duration ÷ the curve's starting slope, `y1` / `x1`.
    /// `exponent` is above -1, so a faster release glides farther.
    /// [`MomentumDecay::ANDROID_FLING`] is Android's `OverScroller`.
    Curve { reference_speed: f64, duration: Duration, exponent: f64, easing: MomentumEasing },
}

/// How one part of a release glides, in the part's unit: logical pixels for
/// pan, zoom levels for zoom, and degrees for bearing and pitch, with speeds
/// per second.
///
/// Each part has a default (`PAN`, `ZOOM`, `BEARING` and `PITCH`), which
/// struct update starts from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MomentumDecay {
    pub model: MomentumDecayModel,
    /// A slower release doesn't glide.
    pub min_speed: f64,
    /// A faster release counts as this speed.
    pub max_speed: f64,
    /// The farthest the part glides. A release that would glide farther
    /// counts as the speed that glides this far.
    pub max_distance: f64,
}

impl MomentumDecay {
    /// Friction with a 140 ms half-life, stopping at 1 unit per second, with
    /// no limits.
    pub(crate) const DEFAULT: MomentumDecay = MomentumDecay {
        model: MomentumDecayModel::Friction { half_life: Duration::from_millis(140), stop_speed: 1.0 },
        min_speed: 0.0,
        max_speed: f64::INFINITY,
        max_distance: f64::INFINITY,
    };
    /// Stops at 10 px/s, needs a release of at least 200 px/s, and counts one
    /// faster than 10 000 px/s as 10 000.
    pub const PAN: MomentumDecay = MomentumDecay {
        model: MomentumDecayModel::Friction { half_life: Duration::from_millis(140), stop_speed: 10.0 },
        min_speed: 200.0,
        max_speed: 10_000.0,
        ..MomentumDecay::DEFAULT
    };
    /// Stops at 0.01 levels/s and glides at most 2.5 levels.
    pub const ZOOM: MomentumDecay = MomentumDecay {
        model: MomentumDecayModel::Friction { half_life: Duration::from_millis(140), stop_speed: 0.01 },
        max_distance: 2.5,
        ..MomentumDecay::DEFAULT
    };
    /// Stops at 1°/s and needs at least 60°/s.
    pub const BEARING: MomentumDecay = MomentumDecay { min_speed: 60.0, ..MomentumDecay::DEFAULT };
    /// Stops at 1°/s and needs at least 5°/s.
    pub const PITCH: MomentumDecay = MomentumDecay { min_speed: 5.0, ..MomentumDecay::DEFAULT };
    /// Android's scroll fling, in its density-independent pixels, which are
    /// logical pixels: its `OverScroller` glides a 1000 dp/s fling about
    /// 194 dp over 555 ms, longer as `speed^0.736`. It needs Android's
    /// minimum fling of 50 dp/s and caps it at Android's maximum, 8000.
    pub const ANDROID_FLING: MomentumDecay = MomentumDecay {
        model: MomentumDecayModel::Curve {
            reference_speed: 1000.0,
            duration: Duration::from_micros(555_182),
            exponent: 0.736_267_6,
            easing: MomentumEasing::ANDROID_FLING,
        },
        min_speed: 50.0,
        max_speed: 8000.0,
        max_distance: f64::INFINITY,
    };

    /// Checks that every number is in range: speeds, distances and durations
    /// positive (`min_speed` may be 0, and the maximums infinite), and a
    /// curve's control points within [0, 1] with a rising start.
    pub fn validate(&self) -> Result<(), InvalidMomentum> {
        let positive = |value: f64| value.is_finite() && value > 0.0;
        let limits =
            self.min_speed.is_finite() && self.min_speed >= 0.0 && self.max_speed > 0.0 && self.max_distance > 0.0;
        let model = match self.model {
            MomentumDecayModel::Friction { half_life, stop_speed } => !half_life.is_zero() && positive(stop_speed),
            MomentumDecayModel::Curve { reference_speed, duration, exponent, easing } => {
                let MomentumEasing { x1, y1, x2, y2 } = easing;
                positive(reference_speed)
                    && !duration.is_zero()
                    && exponent.is_finite()
                    && exponent > -1.0
                    && [x1, y1, x2, y2].iter().all(|value| (0.0..=1.0).contains(value))
                    && x1 > 0.0
                    && y1 > 0.0
            }
        };
        if limits && model {
            Ok(())
        } else {
            Err(InvalidMomentum(
                "a momentum decay needs positive speeds, distances and durations, a minimum speed that isn't \
                 negative, and a curve within [0, 1] that starts rising",
            ))
        }
    }
}

impl Default for MomentumDecay {
    fn default() -> Self {
        MomentumDecay::DEFAULT
    }
}

/// Why a momentum setting is out of range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidMomentum(pub &'static str);

/// Where a part settles once its glide ends: on the nearest of the targets
/// every `spacing` from `offset`, when it is within `tolerance` of one.
/// Bearing targets lie within one turn from `offset`, and the bearing takes
/// the shorter way around to the nearest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MomentumSnap {
    pub spacing: f64,
    pub offset: f64,
    pub tolerance: f64,
    /// Zero settles at once.
    pub duration: Duration,
}

impl MomentumSnap {
    /// The nearest whole level, over 300 ms.
    pub const ZOOM: MomentumSnap =
        MomentumSnap { spacing: 1.0, offset: 0.0, tolerance: 0.5, duration: Duration::from_millis(300) };
    /// North, within 7°, over 300 ms.
    pub const BEARING: MomentumSnap =
        MomentumSnap { spacing: 360.0, offset: 0.0, tolerance: 7.0, duration: Duration::from_millis(300) };
    /// Flat, within 7°, over 300 ms.
    pub const PITCH: MomentumSnap =
        MomentumSnap { spacing: 90.0, offset: 0.0, tolerance: 7.0, duration: Duration::from_millis(300) };

    /// Checks for a positive spacing, a finite offset, and a tolerance that
    /// isn't negative.
    pub fn validate(&self) -> Result<(), InvalidMomentum> {
        if self.spacing.is_finite()
            && self.spacing > 0.0
            && self.offset.is_finite()
            && self.tolerance.is_finite()
            && self.tolerance >= 0.0
        {
            Ok(())
        } else {
            Err(InvalidMomentum(
                "a momentum snap needs a positive spacing, a finite offset, and a tolerance that isn't negative",
            ))
        }
    }
}

/// How one part glides after release: from `velocity`, per second, by
/// `decay`. A pan's decay applies to its speed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MomentumGlide<T> {
    pub(crate) velocity: T,
    pub(crate) decay: MomentumDecay,
}

/// How a gesture's zoom, bearing or pitch keeps changing after release.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AnchoredMomentum {
    /// Without one, the part stops where the gesture leaves it.
    pub(crate) glide: Option<MomentumGlide<f64>>,
    /// The screen point that the change keeps in place; without one, the
    /// center of the view.
    pub(crate) anchor: Option<ScreenCoordinate>,
    /// Where the part settles; without one, it stays where the gesture and
    /// its glide leave it.
    pub(crate) snap: Option<MomentumSnap>,
}

/// Camera motion that continues a gesture after its last contact lifts.
///
/// Each part glides on its own, applied every frame relative to the camera
/// as it stands, so other commands can change the fields that the momentum
/// leaves alone. A command that sets a field stops the parts that move it.
/// On a tilted map, pan moves the map's content the same distance on screen
/// in every direction. [`Default`] glides and settles nothing.
///
/// The momentum's command ends when the momentum does, with a
/// [`TransitionEnd`](crate::camera::TransitionEnd) whose velocity says how fast a
/// cancelled momentum still moved.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CameraMomentum {
    /// Without one, the pan stops where the gesture leaves it.
    pub(crate) pan: Option<MomentumGlide<ScreenCoordinate>>,
    pub(crate) zoom: AnchoredMomentum,
    pub(crate) bearing: AnchoredMomentum,
    pub(crate) pitch: AnchoredMomentum,
}
