//! A gesture recognizer for hosts that have none: pointer, touch, wheel,
//! platform pinch and keyboard input turned into camera moves on a [`mln_map::Map`], through
//! maplibre-native-ffi's camera calls with their gesture phases
//! ([`Map::apply_camera_delta`](mln_map::Map::apply_camera_delta)), and
//! releases continued with the map's momentum.
//!
//! MapLibre Native has no counterpart: each of its platform SDKs recognizes
//! gestures itself. This is ported from Orbis's orb-camera input, which has
//! MapLibre Compose's gesture set, with maplibre-native-ffi's browser demo's
//! rates and release velocities where it has the gesture and GL JS's rates
//! and thresholds elsewhere (the README compares each). It is
//! host-independent: hosts feed [`Input`]s with their timestamps and call
//! [`GestureRecognizer::frame`] once a display frame, and it keeps no clock
//! or thread of its own, so it runs in the browser too.
//!
//! Hosts read two clocks each frame: the recognizer's, in milliseconds on any
//! clock their input events use (`performance.now()`, an `Instant`), which
//! times taps, releases and scrolls; and the map's `Instant::now()`, which they
//! give [`Map::set_frame_time`](mln_map::Map::set_frame_time) and which times
//! the camera's animations. The two never meet, so their epochs don't
//! matter.

mod camera;
mod input;
mod options;
mod recognizer;
mod velocity;

pub use input::{Button, Input, Key, Modifiers, PinchPhase, PointerKind, WheelDelta};
pub use recognizer::GestureRecognizer;
