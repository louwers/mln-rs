// Ports include/mln/map/camera_momentum.hpp, from maplibre-native-ffi's
// patch 0085 (gesture momentum).
//
// On top of the header, the `PAN`, `ZOOM`, `BEARING` and `PITCH` constants
// are maplibre-native-ffi's C API defaults for each part
// (`momentum_defaults` and `resolve_momentum_snap` in its src/map/map.cpp),
// which Orbis's orb-camera also names, so that hosts and the gesture
// recognizer start from the same glide.

use std::time::Duration;

use mln_util::geo::ScreenCoordinate;

/// `MomentumDecay::Model`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MomentumDecayModel {
    /// The speed halves every `half_life`, until it drops to `stop_speed`.
    #[default]
    Friction,
    /// The part moves `distance` over `duration`, along `easing`, as a caller
    /// plans it, for example from a platform's own fling physics.
    Ease,
}

/// `MomentumDecay::Easing`: a unit cubic Bézier, from (0, 0) to (1, 1), with
/// these control points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MomentumEasing {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

impl Default for MomentumEasing {
    fn default() -> Self {
        MomentumEasing { x1: 0.0, y1: 0.0, x2: 0.25, y2: 1.0 }
    }
}

/// How one part of a momentum slows from its release speed.
///
/// Speeds and distances are in the part's unit: logical pixels for pan, zoom
/// levels for zoom, and degrees for bearing and pitch.
///
/// maplibre-native-ffi's C API gives each setting a field bit, and a setting
/// whose bit isn't set takes the part's default. In Rust that is struct
/// update from the part's constant: `UIScrollView`'s normal deceleration with
/// the pan's other defaults is
/// `MomentumDecay { half_life: Duration::from_millis(346), ..MomentumDecay::PAN }`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MomentumDecay {
    pub model: MomentumDecayModel,
    pub half_life: Duration,
    /// Must be positive for [`MomentumDecayModel::Friction`].
    pub stop_speed: f64,
    /// For [`MomentumDecayModel::Ease`].
    pub distance: f64,
    pub duration: Duration,
    /// For [`MomentumDecayModel::Ease`], the share of `distance` covered over
    /// the share of `duration`. Android's scroll fling follows (0.175, 0.5,
    /// 0.35, 1).
    pub easing: MomentumEasing,
    /// A slower release doesn't move.
    pub min_speed: f64,
    /// For [`MomentumDecayModel::Friction`], a faster release counts as this
    /// speed.
    pub max_speed: f64,
    /// The farthest the part moves. A friction release that would move
    /// farther starts slower, so it ends sooner, and an eased glide moves at
    /// most this far over its duration.
    pub max_distance: f64,
}

impl MomentumDecay {
    /// The header's defaults: friction with a 140 ms half-life, stopping at
    /// 1 unit per second, with no limits.
    pub const FRICTION: MomentumDecay = MomentumDecay {
        model: MomentumDecayModel::Friction,
        half_life: Duration::from_millis(140),
        stop_speed: 1.0,
        distance: 0.0,
        duration: Duration::ZERO,
        easing: MomentumEasing { x1: 0.0, y1: 0.0, x2: 0.25, y2: 1.0 },
        min_speed: 0.0,
        max_speed: f64::INFINITY,
        max_distance: f64::INFINITY,
    };
    /// maplibre-native-ffi's pan default: stops at 10 px/s, needs a release
    /// of at least 200 px/s, and counts one faster than 10 000 px/s as
    /// 10 000.
    pub const PAN: MomentumDecay =
        MomentumDecay { stop_speed: 10.0, min_speed: 200.0, max_speed: 10_000.0, ..MomentumDecay::FRICTION };
    /// maplibre-native-ffi's zoom default: stops at 0.01 levels/s and moves at
    /// most 2.5 levels.
    pub const ZOOM: MomentumDecay = MomentumDecay { stop_speed: 0.01, max_distance: 2.5, ..MomentumDecay::FRICTION };
    /// maplibre-native-ffi's bearing default: stops at 1°/s and needs at least
    /// 60°/s.
    pub const BEARING: MomentumDecay = MomentumDecay { min_speed: 60.0, ..MomentumDecay::FRICTION };
    /// maplibre-native-ffi's pitch default: stops at 1°/s and needs at least
    /// 5°/s.
    pub const PITCH: MomentumDecay = MomentumDecay { min_speed: 5.0, ..MomentumDecay::FRICTION };
}

impl Default for MomentumDecay {
    fn default() -> Self {
        MomentumDecay::FRICTION
    }
}

/// Where a part settles once its glide ends normally: on the nearest of the
/// targets every `spacing` from `offset`, when it is within `tolerance` of
/// one. Bearing targets lie within one turn from `offset`, and the bearing
/// takes the shorter way around to the nearest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MomentumSnap {
    pub spacing: f64,
    pub offset: f64,
    pub tolerance: f64,
    /// Zero settles at once.
    pub duration: Duration,
}

impl MomentumSnap {
    /// maplibre-native-ffi's zoom snap: the nearest whole level, over 300 ms.
    pub const ZOOM: MomentumSnap =
        MomentumSnap { spacing: 1.0, offset: 0.0, tolerance: 0.5, duration: Duration::from_millis(300) };
    /// maplibre-native-ffi's bearing snap: north, within 7°, over 300 ms.
    pub const BEARING: MomentumSnap =
        MomentumSnap { spacing: 360.0, offset: 0.0, tolerance: 7.0, duration: Duration::from_millis(300) };
    /// maplibre-native-ffi's pitch snap: flat, within 7°, over 300 ms.
    pub const PITCH: MomentumSnap =
        MomentumSnap { spacing: 90.0, offset: 0.0, tolerance: 7.0, duration: Duration::from_millis(300) };
}

impl Default for MomentumSnap {
    fn default() -> Self {
        MomentumSnap { spacing: 360.0, offset: 0.0, tolerance: 7.0, duration: Duration::ZERO }
    }
}

/// Camera motion that continues a gesture after its last contact lifts.
///
/// Each part starts at its release velocity and slows by its decay, applied
/// every frame relative to the camera as it stands, so other commands can
/// change the fields that the momentum leaves alone. A command that sets a
/// field stops the parts that move it. On a tilted map, pan moves the map's
/// content the same distance on screen in every direction.
///
/// [`Default`] gives each part maplibre-native-ffi's C API default decay
/// ([`MomentumDecay::PAN`], [`ZOOM`](MomentumDecay::ZOOM),
/// [`BEARING`](MomentumDecay::BEARING) and [`PITCH`](MomentumDecay::PITCH)),
/// and no velocities, anchors or snaps: a part moves only when its velocity
/// is set, and settles only when its snap is.
///
/// The momentum ends through its command's
/// [`AnimationOptions::transition_end_fn`](crate::AnimationOptions::transition_end_fn),
/// whose velocity says how fast a cancelled momentum still moved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraMomentum {
    /// How fast the map's content moved on screen, in logical pixels per
    /// second.
    pub pan_velocity: Option<ScreenCoordinate>,
    pub pan_decay: MomentumDecay,

    /// Zoom levels per second.
    pub zoom_velocity: Option<f64>,
    pub zoom_decay: MomentumDecay,
    /// The screen point that the zoom keeps in place.
    pub zoom_anchor: Option<ScreenCoordinate>,
    pub zoom_snap: Option<MomentumSnap>,

    /// Degrees per second.
    pub bearing_velocity: Option<f64>,
    pub bearing_decay: MomentumDecay,
    /// The screen point that the rotation keeps in place.
    pub bearing_anchor: Option<ScreenCoordinate>,
    pub bearing_snap: Option<MomentumSnap>,

    /// Degrees per second.
    pub pitch_velocity: Option<f64>,
    pub pitch_decay: MomentumDecay,
    /// The screen point that the tilt keeps in place.
    pub pitch_anchor: Option<ScreenCoordinate>,
    pub pitch_snap: Option<MomentumSnap>,
}

impl Default for CameraMomentum {
    fn default() -> Self {
        CameraMomentum {
            pan_velocity: None,
            pan_decay: MomentumDecay::PAN,
            zoom_velocity: None,
            zoom_decay: MomentumDecay::ZOOM,
            zoom_anchor: None,
            zoom_snap: None,
            bearing_velocity: None,
            bearing_decay: MomentumDecay::BEARING,
            bearing_anchor: None,
            bearing_snap: None,
            pitch_velocity: None,
            pitch_decay: MomentumDecay::PITCH,
            pitch_anchor: None,
            pitch_snap: None,
        }
    }
}
