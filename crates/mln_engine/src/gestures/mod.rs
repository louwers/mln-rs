//! A gesture recognizer that every host shares: pointer, touch, platform
//! pinch and other camera input turned into camera moves on a
//! [`GestureCamera`], such as `mln_runtime::Map`, through the map's camera
//! calls with their gesture phases
//! ([`Map::apply_camera_delta`](crate::map::Map::apply_camera_delta)), and
//! releases continued with the map's momentum. Hosts translate each platform
//! event into an [`Input`]: which buttons and keys select which [`Modes`],
//! and how far a wheel, a dial or a key moves the camera. The recognizer
//! interprets them over time, and reports what gestures mean beyond camera
//! moves as [`GestureEvent`]s: taps, which hosts answer, and the notches
//! where hosts play haptic feedback. [`GestureOptions`] sets, for each part
//! of the camera, whether gestures move it, how its release glides, where it
//! settles, and where it has notches.
//!
//! Hosts feed [`Input`]s with their timestamps and call
//! [`GestureRecognizer::tick`] when it asks. The recognizer keeps no clock
//! or thread of its own.
//!
//! Hosts read two clocks each frame: the recognizer's, in milliseconds on any
//! clock their input events use (`performance.now()`, an `Instant`), which
//! times taps, releases and scrolls; and the map's `Instant::now()`, which they
//! give the map's frame time (`mln_runtime::RenderSession`'s frames) and which times
//! the camera's animations. The two never meet, so their epochs don't
//! matter.

mod camera;
mod input;
mod notches;
mod options;
mod recognizer;
mod velocity;

pub use camera::{GestureCamera, PendingEnd, zoom_range};
pub use input::{GestureEvent, Input, Modes, PinchPhase, PointerKind};
pub use options::{Axis, AxisOptions, GestureOptions, InvalidGestureOptions, Notch, PanOptions};
pub use recognizer::{GestureRecognizer, NextTick};
