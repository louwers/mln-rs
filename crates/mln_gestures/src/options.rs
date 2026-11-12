//! The recognizer's rates, thresholds and releases.

use mln_map::{MomentumDecay, MomentumSnap};

/// How the recognizer turns input into camera moves. The defaults are
/// maplibre-native-ffi's browser demo's rates where it has the gesture, GL
/// JS's handlers' rates and thresholds where it doesn't, and ffi's C API
/// momentum defaults. The crate's README compares each with ffi, Orbis,
/// Compose and GL JS.
///
/// Distances are in logical pixels, and times in milliseconds (`_ms`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GestureOptions {
    /// How far a press may move and still be a click; drags start past it.
    pub click_tolerance: f64,
    /// How far apart two taps of a double tap may be, and the most time
    /// between them. A two-finger tap's fingers may move this far.
    pub tap_tolerance: f64,
    pub tap_interval_ms: f64,
    /// Degrees of bearing per pixel of a sideways rotate drag: ffi's demo's
    /// 0.5 (GL JS, Compose and Orbis turn 0.8).
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
    /// Zoom levels per wheel notch, toward the user zooming in, and how long
    /// a notch eases: ffi's demo's and Compose's 100 px at 1/300 per px (⅓),
    /// over GL JS's 200 ms. Provisional: unlike glides, no one has measured
    /// the native map apps' notches yet (see the README).
    pub notch_zoom: f64,
    pub notch_duration_ms: f64,
    /// Zoom levels per pixel of continuous scroll, toward the user zooming
    /// in: ffi's demo's and Compose's 1/300. Provisional, as `notch_zoom`.
    pub scroll_zoom_rate: f64,
    /// The same with Ctrl held, as a browser's trackpad pinch sends: ffi's
    /// demo's 1/100.
    pub pinch_zoom_rate: f64,
    /// The most a scroll zooms in one step, in levels: GL JS's
    /// `maxScalePerFrame` of 2. A notch carries the rest to later frames.
    pub max_scroll_zoom: f64,
    /// A scroll is one gesture until this long after its last event and its
    /// eases (Compose's and GL JS's 200 ms).
    pub scroll_idle_ms: f64,
    /// Keyboard steps: pixels of pan, degrees of bearing, degrees of pitch.
    pub key_pan: f64,
    pub key_bearing: f64,
    pub key_pitch: f64,

    /// How each part of a release slows down.
    pub pan_decay: MomentumDecay,
    pub zoom_decay: MomentumDecay,
    pub bearing_decay: MomentumDecay,
    pub pitch_decay: MomentumDecay,
    /// Where each part settles after a gesture that changed it, as
    /// maplibre-native-ffi's `mln_anchored_momentum::snap` and Compose's
    /// `snapping` set it. The defaults are Compose's: the bearing settles on
    /// north within 7°, and zoom and pitch stay where the gesture leaves
    /// them ([`MomentumSnap::ZOOM`] and [`MomentumSnap::PITCH`] are ffi's
    /// C API defaults, which settle them on a whole level and flat).
    pub zoom_snap: Option<MomentumSnap>,
    pub bearing_snap: Option<MomentumSnap>,
    pub pitch_snap: Option<MomentumSnap>,

    /// Whether a release that catches the recognizer's last glide (a flick
    /// that interrupts a glide going its way) adds that glide's speed, as
    /// `UIScrollView`, Android's scroll flywheel and Compose do. Each part's
    /// decay still caps the speed. No map app measured in
    /// maplibre-native-ffi#1705 does it (Apple Maps, Google Maps), nor do GL
    /// JS and ffi's demo.
    pub carry_caught_glide: bool,
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
        tap_interval_ms: 500.0,
        drag_rotate: 0.5,
        drag_pitch: 0.5,
        tap_drag_zoom: 1.0 / 128.0,
        pinch_zoom_threshold: 0.1,
        rotate_threshold: 25.0,
        notch_zoom: 1.0 / 3.0,
        notch_duration_ms: 200.0,
        scroll_zoom_rate: 1.0 / 300.0,
        pinch_zoom_rate: 1.0 / 100.0,
        max_scroll_zoom: 1.0,
        scroll_idle_ms: 200.0,
        key_pan: 100.0,
        key_bearing: 15.0,
        key_pitch: 10.0,
        pan_decay: MomentumDecay::PAN,
        zoom_decay: MomentumDecay::ZOOM,
        bearing_decay: MomentumDecay::BEARING,
        pitch_decay: MomentumDecay::PITCH,
        zoom_snap: None,
        bearing_snap: Some(MomentumSnap::BEARING),
        pitch_snap: None,
        carry_caught_glide: true,
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
