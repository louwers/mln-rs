// The integration tests, in one binary rather than one per file: each binary
// links the renderer and every backend, which makes many large ones.

mod support;

mod camera;
mod commands;
mod devices;
mod driver;
mod events;
mod exported;
mod feature_state;
mod frame_depth;
mod gestures;
mod resources;
mod sessions;
mod style;
mod surfaces;
mod surfaces_win32;
mod surfaces_window;
mod teardown;
