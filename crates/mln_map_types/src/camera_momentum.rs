// The `PAN`, `ZOOM`, `BEARING` and `PITCH` constants are the C API's defaults
// for each part, so that hosts and the gesture recognizer start from the same
// glide.

use std::time::Duration;

use mln_util::geo::ScreenCoordinate;

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
    pub const DEFAULT: MomentumEasing = MomentumEasing { x1: 0.0, y1: 0.0, x2: 0.25, y2: 1.0 };
}

impl Default for MomentumEasing {
    fn default() -> Self {
        MomentumEasing::DEFAULT
    }
}

/// How a part slows from its release speed by friction: the speed halves
/// every `half_life`, until it drops to `stop_speed`.
///
/// Speeds and distances are in the part's unit: logical pixels for pan, zoom
/// levels for zoom, and degrees for bearing and pitch. `UIScrollView`'s
/// normal deceleration is a half-life of 346 ms, and its fast deceleration
/// 69 ms. Each part has a default (`PAN`, `ZOOM`, `BEARING` and `PITCH`),
/// which struct update starts from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MomentumFriction {
    pub half_life: Duration,
    /// Must be positive.
    pub stop_speed: f64,
    /// A slower release doesn't glide.
    pub min_speed: f64,
    /// A faster release counts as this speed.
    pub max_speed: f64,
    /// The farthest the part glides. A release that would glide farther
    /// starts slower, so it ends sooner.
    pub max_distance: f64,
}

impl MomentumFriction {
    /// A 140 ms half-life, stopping at 1 unit per second, with no limits.
    pub const DEFAULT: MomentumFriction = MomentumFriction {
        half_life: Duration::from_millis(140),
        stop_speed: 1.0,
        min_speed: 0.0,
        max_speed: f64::INFINITY,
        max_distance: f64::INFINITY,
    };
    /// Stops at 10 px/s, needs a release of at least 200 px/s, and counts one
    /// faster than 10 000 px/s as 10 000.
    pub const PAN: MomentumFriction =
        MomentumFriction { stop_speed: 10.0, min_speed: 200.0, max_speed: 10_000.0, ..MomentumFriction::DEFAULT };
    /// Stops at 0.01 levels/s and glides at most 2.5 levels.
    pub const ZOOM: MomentumFriction =
        MomentumFriction { stop_speed: 0.01, max_distance: 2.5, ..MomentumFriction::DEFAULT };
    /// Stops at 1°/s and needs at least 60°/s.
    pub const BEARING: MomentumFriction = MomentumFriction { min_speed: 60.0, ..MomentumFriction::DEFAULT };
    /// Stops at 1°/s and needs at least 5°/s.
    pub const PITCH: MomentumFriction = MomentumFriction { min_speed: 5.0, ..MomentumFriction::DEFAULT };
}

impl Default for MomentumFriction {
    fn default() -> Self {
        MomentumFriction::DEFAULT
    }
}

/// How one part glides after release, in the part's unit: a
/// [`ScreenCoordinate`] in logical pixels for pan, and an `f64` for zoom
/// (levels), bearing and pitch (degrees).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MomentumGlide<T> {
    /// From the release velocity, per second, slowing by friction. A pan's
    /// friction applies to its speed.
    Friction { velocity: T, friction: MomentumFriction },
    /// As the caller planned it, for example from a platform's own fling
    /// physics: the part changes by `delta` over `duration`, along `easing`,
    /// whose control points are within [0, 1] so the glide never passes
    /// `delta`.
    Planned { delta: T, duration: Duration, easing: MomentumEasing },
}

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
}

/// How a gesture's zoom, bearing or pitch keeps changing after release.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AnchoredMomentum {
    /// Without one, the part stops where the gesture leaves it.
    pub glide: Option<MomentumGlide<f64>>,
    /// The screen point that the change keeps in place; without one, the
    /// center of the view.
    pub anchor: Option<ScreenCoordinate>,
    /// Where the part settles; without one, it stays where the gesture and
    /// its glide leave it.
    pub snap: Option<MomentumSnap>,
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
/// [`TransitionEnd`](crate::TransitionEnd) whose velocity says how fast a
/// cancelled momentum still moved.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CameraMomentum {
    /// Without one, the pan stops where the gesture leaves it.
    pub pan: Option<MomentumGlide<ScreenCoordinate>>,
    pub zoom: AnchoredMomentum,
    pub bearing: AnchoredMomentum,
    pub pitch: AnchoredMomentum,
}
