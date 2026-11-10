//! The recognizer's rates, thresholds and releases.

use mln_map::{MomentumDecay, MomentumSnap};

/// How the recognizer turns input into camera moves. The defaults are GL JS's
/// handlers' rates and thresholds, and maplibre-native-ffi's momentum
/// defaults.
///
/// Distances are in logical pixels and times in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GestureOptions {
    /// How far a press may move and still be a click; drags start past it.
    pub click_tolerance: f64,
    /// How far apart two taps of a double tap may be, and the most time
    /// between them. A two-finger tap's fingers may move this far.
    pub tap_tolerance: f64,
    pub tap_interval: f64,
    /// Degrees of bearing per pixel of a sideways rotate drag.
    pub drag_rotate: f64,
    /// Degrees of pitch per pixel of an upward rotate drag or two-finger
    /// drag.
    pub drag_pitch: f64,
    /// Zoom levels per pixel of a double-tap drag downward.
    pub tap_drag_zoom: f64,
    /// Zoom levels a pinch spreads or closes before it zooms.
    pub pinch_zoom_threshold: f64,
    /// Pixels along the fingers' circle a twist travels before it rotates.
    pub rotate_threshold: f64,
    /// Wheel zoom rates per pixel of delta: for a mouse wheel's large steps,
    /// and for trackpads and small steps.
    pub wheel_zoom_rate: f64,
    pub trackpad_zoom_rate: f64,
    /// Keyboard steps: pixels of pan, degrees of bearing, degrees of pitch.
    pub key_pan: f64,
    pub key_bearing: f64,
    pub key_pitch: f64,

    /// How each part of a release slows down.
    pub pan_decay: MomentumDecay,
    pub zoom_decay: MomentumDecay,
    pub bearing_decay: MomentumDecay,
    pub pitch_decay: MomentumDecay,
    /// Where the bearing settles after a gesture that turned the map.
    pub bearing_snap: Option<MomentumSnap>,

    /// Whether a press stops every camera animation, as GL JS and the
    /// MapLibre Native SDKs do, or only the recognizer's own.
    pub interrupt_animations: bool,
    /// The [`AnimationOptions::transition_id`](mln_map::AnimationOptions::transition_id)
    /// of the eases and momentum the recognizer starts.
    pub transition_id: u64,
}

impl GestureOptions {
    pub const DEFAULT: GestureOptions = GestureOptions {
        click_tolerance: 3.0,
        tap_tolerance: 30.0,
        tap_interval: 500.0,
        drag_rotate: 0.8,
        drag_pitch: 0.5,
        tap_drag_zoom: 1.0 / 128.0,
        pinch_zoom_threshold: 0.1,
        rotate_threshold: 25.0,
        wheel_zoom_rate: 1.0 / 450.0,
        trackpad_zoom_rate: 1.0 / 100.0,
        key_pan: 100.0,
        key_bearing: 15.0,
        key_pitch: 10.0,
        pan_decay: MomentumDecay::PAN,
        zoom_decay: MomentumDecay::ZOOM,
        bearing_decay: MomentumDecay::BEARING,
        pitch_decay: MomentumDecay::PITCH,
        bearing_snap: Some(MomentumSnap::BEARING),
        interrupt_animations: true,
        // "gestures" in ASCII.
        transition_id: 0x6765_7374_7572_6573,
    };
}

impl Default for GestureOptions {
    fn default() -> Self {
        GestureOptions::DEFAULT
    }
}
