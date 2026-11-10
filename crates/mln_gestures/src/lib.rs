//! A gesture recognizer for hosts that have none: pointer, touch, wheel and
//! keyboard input turned into camera moves on a [`mln_map::Map`], and
//! releases continued with the map's momentum
//! ([`Map::start_momentum`](mln_map::Map::start_momentum)).
//!
//! MapLibre Native has no counterpart: each of its platform SDKs recognizes
//! gestures itself. This is ported from Orbis's orb-camera input, which has
//! MapLibre Compose's gesture set with GL JS's rates and thresholds. It is
//! host-independent: hosts feed [`Input`]s with their timestamps and call
//! [`GestureRecognizer::frame`] once a display frame, and it keeps no clock
//! or thread of its own, so it runs in the browser too.

mod camera;
mod input;
mod options;
mod recognizer;
mod velocity;

pub use camera::GestureCamera;
pub use input::{Button, GestureEvent, Input, Key, Modifiers, PointerKind, WheelUnit};
pub use options::GestureOptions;
pub use recognizer::GestureRecognizer;
