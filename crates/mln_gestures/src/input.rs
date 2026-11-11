//! What hosts feed the recognizer, and the gestures it reports back.

use mln_util::geo::ScreenCoordinate;

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
    /// The view lost focus: every pointer is released.
    Blur,
}

/// What a gesture means beyond moving the camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GestureEvent {
    /// A press and release that didn't move the map: a click or a tap.
    Click { position: ScreenCoordinate, button: Button },
    /// The second click or tap of a double click or double tap.
    DoubleClick { position: ScreenCoordinate },
}
