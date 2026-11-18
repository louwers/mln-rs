//! What hosts feed the recognizer, and the gestures it reports back.

use mln_util::geo::ScreenCoordinate;

use crate::options::Axis;

/// What kind of device a pointer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerKind {
    Mouse,
    Touch,
    Pen,
}

/// The pointer button that went down. Touches and pens are `Primary`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Primary,
    Secondary,
    Middle,
}

/// The modifier keys held with an input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

/// The keys the map responds to; hosts map platform key codes to them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    /// `+` or `=`.
    ZoomIn,
    /// `-`.
    ZoomOut,
    /// Cancels a box zoom.
    Escape,
}

/// How far a scroll went, as its host classifies it. Positive scrolls down,
/// as the DOM's `deltaY` does, which zooms out.
///
/// The host knows best which device scrolled (macOS's
/// `hasPreciseScrollingDeltas`, the DOM's `deltaMode`, the platform's wheel
/// units), so the recognizer doesn't guess from the deltas' sizes and timing,
/// which GL JS does and which read a slow trackpad start as a wheel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WheelDelta {
    /// Detents of a mouse wheel, fractional for high-resolution wheels: each
    /// zooms by a fixed amount, eased.
    Notches(f64),
    /// Continuous scrolling, as trackpads and precise wheels send, in logical
    /// pixels: zooms at once, as the platform already smoothed it.
    Pixels(f64),
}

/// One input, in logical pixels from the viewport's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    PointerDown {
        id: u32,
        kind: PointerKind,
        button: Button,
        position: ScreenCoordinate,
        modifiers: Modifiers,
    },
    PointerMove {
        id: u32,
        position: ScreenCoordinate,
    },
    PointerUp {
        id: u32,
        position: ScreenCoordinate,
    },
    /// The platform took the pointer away (a system gesture, lost capture):
    /// no click and no momentum.
    PointerCancel {
        id: u32,
    },
    /// A scroll at `position`. With Ctrl, which browsers add to a trackpad
    /// pinch, pixels zoom at the pinch rate.
    Wheel {
        position: ScreenCoordinate,
        delta: WheelDelta,
        modifiers: Modifiers,
    },
    Key {
        key: Key,
        modifiers: Modifiers,
    },
    /// A pinch the platform recognized, such as a macOS trackpad's magnify
    /// and rotate: each update zooms by `scale` (above 1 spreads the
    /// fingers, zooming in) and turns `rotation` degrees clockwise on screen
    /// since the last, around `center`. It glides on when it ends, as a touch
    /// pinch does. An update without a begin begins; pointers already down
    /// keep the map, and a touch that goes down takes over from the pinch.
    Pinch {
        center: ScreenCoordinate,
        scale: f64,
        rotation: f64,
        phase: PinchPhase,
    },
    /// The view lost focus: every pointer is released.
    Blur,
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

/// What a gesture means beyond moving the camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GestureEvent {
    /// A press and release that didn't move the map: a click or a tap.
    Click { position: ScreenCoordinate, button: Button },
    /// The second click or tap of a double click or double tap.
    DoubleClick { position: ScreenCoordinate },
    /// A gesture, its glide or its snap moved `axis` to or across the
    /// [`Notch`](crate::Notch) of `kind` at `value`, in the axis's unit, a
    /// bearing in [0°, 360°): the host plays its feedback. A gesture that
    /// starts at a notch reports it only once it leaves and comes back, an
    /// axis reports at most one notch every 50 ms, and keys, double taps and
    /// box zooms report none unless they step within a gesture under way.
    Notch { axis: Axis, value: f64, kind: u32 },
}
