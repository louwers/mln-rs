//! The recognizer's rates, thresholds, and how gestures move each part of
//! the camera.

use mln_map::{MomentumFriction, MomentumSnap};

/// Values every `spacing` from `offset`, in the part's unit, that report a
/// [`GestureEvent::Notch`](crate::GestureEvent::Notch) as a gesture moves the
/// part to or across one, for the host to play as haptic feedback. Bearing
/// notches wrap around the circle.
///
/// The recognizer ignores a notch unless its spacing is positive and finite
/// and its offset is finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Notch {
    pub spacing: f64,
    pub offset: f64,
    /// The host's name for the feedback, such as which haptic to play, which
    /// the notch's events carry.
    pub kind: u32,
}

impl Notch {
    /// North, as MapLibre Compose ticks by default.
    pub const NORTH: Notch = Notch { spacing: 360.0, offset: 0.0, kind: 0 };
}

/// How gestures move the map's content.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanOptions {
    /// Whether gestures pan: drags, two fingers moving together, and the
    /// arrow keys.
    pub enabled: bool,
    /// How a release's glide slows; without it, the pan stops where the
    /// gesture leaves it.
    pub momentum: Option<MomentumFriction>,
}

/// How gestures move the zoom, the bearing or the pitch.
#[derive(Clone, Debug, PartialEq)]
pub struct AxisOptions {
    /// Whether gestures move the part.
    pub enabled: bool,
    /// How a release's glide slows; without it, the part stops where the
    /// gesture leaves it.
    pub momentum: Option<MomentumFriction>,
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

/// How the recognizer turns input into camera moves. The defaults are
/// maplibre-native-ffi's browser demo's rates where it has the gesture, GL
/// JS's handlers' rates and thresholds where it doesn't, and MapLibre
/// Compose's momentum, snaps and notches. The crate's README compares each
/// with ffi, Orbis, Compose and GL JS.
///
/// Distances are in logical pixels, and times in milliseconds (`_ms`).
#[derive(Clone, Debug, PartialEq)]
pub struct GestureOptions {
    /// Each part of the camera: the C API's default friction; the bearing
    /// settles on north within 7° and notches there; zoom and pitch stay
    /// where gestures leave them, without notches
    /// ([`MomentumSnap::ZOOM`] and [`MomentumSnap::PITCH`] settle them on a
    /// whole level and flat).
    pub pan: PanOptions,
    pub zoom: AxisOptions,
    pub bearing: AxisOptions,
    pub pitch: AxisOptions,
    /// Whether a wheel or trackpad scroll zooms. Off, the recognizer leaves
    /// scrolls unhandled.
    pub scroll_zoom: bool,

    /// How far a press may move and still be a click; drags start past it.
    pub(crate) click_tolerance: f64,
    /// How far apart two taps of a double tap may be, and the most time
    /// between them. A two-finger tap's fingers may move this far.
    pub(crate) tap_tolerance: f64,
    pub(crate) tap_interval_ms: f64,
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
    /// Zoom levels per wheel notch, toward the user zooming in, and how long
    /// a notch eases: ffi's demo's and Compose's 100 px at 1/300 per px (⅓),
    /// over GL JS's 200 ms. Provisional: unlike glides, no one has measured
    /// the native map apps' notches yet (see the README).
    pub(crate) notch_zoom: f64,
    pub(crate) notch_duration_ms: f64,
    /// Zoom levels per pixel of continuous scroll, toward the user zooming
    /// in: ffi's demo's and Compose's 1/300. Provisional, as `notch_zoom`.
    pub(crate) scroll_zoom_rate: f64,
    /// The same with Ctrl held, as a browser's trackpad pinch sends: ffi's
    /// demo's 1/100.
    pub(crate) pinch_zoom_rate: f64,
    /// The most a scroll zooms in one step, in levels: GL JS's
    /// `maxScalePerFrame` of 2. A notch carries the rest to later frames.
    pub(crate) max_scroll_zoom: f64,
    /// A scroll is one gesture until this long after its last event and its
    /// eases (Compose's and GL JS's 200 ms).
    pub(crate) scroll_idle_ms: f64,
    /// Keyboard steps: pixels of pan, degrees of bearing, degrees of pitch.
    pub(crate) key_pan: f64,
    pub(crate) key_bearing: f64,
    pub(crate) key_pitch: f64,

    /// Whether a release that catches the recognizer's last glide (a flick
    /// that interrupts a glide going its way) adds that glide's speed, as
    /// `UIScrollView`, Android's scroll flywheel and Compose do. Each part's
    /// friction still caps the speed. No map app measured in
    /// maplibre-native-ffi#1705 does it (Apple Maps, Google Maps), nor do GL
    /// JS and ffi's demo.
    pub(crate) carry_caught_glide: bool,
    /// Whether a press stops every camera animation, as GL JS and the
    /// MapLibre Native SDKs do, or only the recognizer's own.
    pub(crate) interrupt_animations: bool,
    /// The [`AnimationOptions::transition_id`](mln_map::AnimationOptions::transition_id)
    /// of the eases and momentum the recognizer starts.
    pub(crate) transition_id: u64,
}

impl GestureOptions {
    /// How far a press may move and still be a click, in logical pixels.
    #[must_use]
    pub fn click_tolerance(&self) -> f64 {
        self.click_tolerance
    }

    /// The options of `axis`.
    #[must_use]
    pub fn axis(&self, axis: Axis) -> &AxisOptions {
        match axis {
            Axis::Zoom => &self.zoom,
            Axis::Bearing => &self.bearing,
            Axis::Pitch => &self.pitch,
        }
    }
}

impl Default for GestureOptions {
    fn default() -> Self {
        let axis = |momentum: MomentumFriction, snap: Option<MomentumSnap>, notches: Vec<Notch>| AxisOptions {
            enabled: true,
            momentum: Some(momentum),
            snap,
            notches,
        };
        GestureOptions {
            pan: PanOptions { enabled: true, momentum: Some(MomentumFriction::PAN) },
            zoom: axis(MomentumFriction::ZOOM, None, Vec::new()),
            bearing: axis(MomentumFriction::BEARING, Some(MomentumSnap::BEARING), vec![Notch::NORTH]),
            pitch: axis(MomentumFriction::PITCH, None, Vec::new()),
            scroll_zoom: true,
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
            carry_caught_glide: true,
            interrupt_animations: true,
            // "gestures" in ASCII.
            transition_id: 0x6765_7374_7572_6573,
        }
    }
}
