//! What hosts feed the recognizer, and the gestures it reports back.
//!
//! Hosts translate each platform event on its own, with no memory: which
//! buttons and keys mean which [`Modes`], and how far a wheel, dial or key
//! moves the camera ([`Input::Delta`]). The recognizer interprets them over
//! time.

use std::ops::BitOr;

use crate::geo::ScreenCoordinate;
use crate::gestures::options::Axis;

/// What kind of device a pointer is. Touches pinch, twist and tilt with two
/// fingers, and a held second tap zooms as it drags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerKind {
    Mouse,
    Touch,
    Pen,
}

/// What a one-pointer drag or a double tap does instead of its default, from
/// the pointer's held buttons (a mouse's secondary button rotates on most
/// platforms) or from held keys ([`Input::Modes`]). A drag reads the modes
/// when it starts and keeps them until it ends, and a double tap when it
/// lifts; rotating wins over a box zoom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modes {
    /// Sideways turns the map and up and down tilts it.
    pub rotate: bool,
    /// Draws a box that the map zooms to when the pointer lifts.
    pub box_zoom: bool,
    /// A double tap zooms out instead of in.
    pub zoom_out: bool,
}

impl BitOr for Modes {
    type Output = Modes;

    fn bitor(self, other: Modes) -> Modes {
        Modes {
            rotate: self.rotate || other.rotate,
            box_zoom: self.box_zoom || other.box_zoom,
            zoom_out: self.zoom_out || other.zoom_out,
        }
    }
}

/// One input, in logical pixels from the viewport's top-left corner.
///
/// When the view loses focus, hosts cancel the pointers they sent and send
/// empty modes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    /// A pointer whose buttons the host maps to `modes`. Hosts send no
    /// pointer whose buttons mean nothing, such as a mouse's middle button.
    PointerDown {
        id: u32,
        kind: PointerKind,
        position: ScreenCoordinate,
        modes: Modes,
    },
    PointerMove {
        id: u32,
        position: ScreenCoordinate,
    },
    PointerUp {
        id: u32,
        position: ScreenCoordinate,
    },
    /// The platform or the host took the pointer away (a system gesture,
    /// lost capture, Escape during a box zoom): no tap and no momentum.
    PointerCancel {
        id: u32,
    },
    /// The modes that held keys select, for every pointer.
    Modes(Modes),
    /// Camera movement from anything but a pointer (a wheel, a trackpad
    /// scroll, a dial, a crown, keys, buttons), which the host converts into
    /// camera units: `pan` moves the map's content on screen in logical
    /// pixels, `zoom` adds levels, `bearing` turns clockwise in degrees, and
    /// `pitch` tilts in degrees. Zoom and turns hold `position` in place.
    ///
    /// A continuous delta (a trackpad scroll) applies at once. A discrete one
    /// (a wheel detent, a key press) eases, and a quick run of them merges
    /// into one ease that speeds up. Deltas are one gesture until they idle.
    Delta {
        position: ScreenCoordinate,
        discrete: bool,
        pan: ScreenCoordinate,
        zoom: f64,
        bearing: f64,
        pitch: f64,
    },
    /// A pinch the platform recognized, such as a macOS trackpad's magnify
    /// and rotate or a browser's Ctrl+wheel: each update zooms by `scale`
    /// (above 1 spreads the fingers, zooming in) and turns `rotation` degrees
    /// clockwise on screen since the last, around `center`. It glides on when
    /// it ends, as a touch pinch does. An update without a begin begins;
    /// pointers already down keep the map, and a touch that goes down takes
    /// over from the pinch.
    Pinch {
        center: ScreenCoordinate,
        scale: f64,
        rotation: f64,
        phase: PinchPhase,
    },
}

/// Where a platform-recognized pinch is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PinchPhase {
    Begin,
    Update,
    /// The fingers lifted: the pinch glides on.
    End,
    /// The platform took the pinch away: nothing continues, and the camera's
    /// animations stop.
    Cancel,
}

/// What a gesture means beyond moving the camera. Double taps and two-finger
/// taps also zoom, as the options say.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GestureEvent {
    /// The pointer `id` pressed and lifted without moving the map, before a
    /// long press.
    Tap { id: u32, position: ScreenCoordinate },
    /// The pointer `id` stayed pressed and still for 500 ms; its lift makes
    /// no tap.
    LongPress { id: u32, position: ScreenCoordinate },
    /// The second tap of a double tap, by pointer `id`.
    DoubleTap { id: u32, position: ScreenCoordinate },
    /// Two fingers tapped together, around `position`.
    TwoFingerTap { position: ScreenCoordinate },
    /// A gesture, its glide or its snap moved `axis` to or across the
    /// [`Notch`](crate::gestures::Notch) of `kind` at `value`, in the axis's unit, a
    /// bearing in [0°, 360°): the host plays its feedback. A gesture that
    /// starts at a notch reports it only once it leaves and comes back, an
    /// axis reports at most one notch every 50 ms, and box zooms report none.
    Notch { axis: Axis, value: f64, kind: u32 },
}
