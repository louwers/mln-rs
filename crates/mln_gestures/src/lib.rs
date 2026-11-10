//! A gesture recognizer for hosts that have none: pointer, touch, wheel and
//! keyboard input turned into camera moves on a [`mln_map::Map`], and
//! releases continued with the map's momentum
//! ([`Map::start_momentum`](mln_map::Map::start_momentum)).
//!
//! MapLibre Native has no counterpart: each of its platform SDKs recognizes
//! gestures itself. This is ported from Orbis's orb-camera input, which has
//! MapLibre Compose's gesture set, with maplibre-native-ffi's browser demo's
//! rates and release velocities where it has the gesture and GL JS's rates
//! and thresholds elsewhere (the README compares each). It is
//! host-independent: hosts feed [`Input`]s with their timestamps and call
//! [`GestureRecognizer::frame`] once a display frame, and it keeps no clock
//! or thread of its own, so it runs in the browser too.

mod camera;
mod input;
mod options;
mod recognizer;
mod velocity;

pub use camera::GestureCamera;
pub use input::{Button, GestureEvent, Input, Key, Modifiers, PointerKind, WheelDelta};
pub use options::GestureOptions;
pub use recognizer::GestureRecognizer;
