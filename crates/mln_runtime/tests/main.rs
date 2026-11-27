// The integration tests, in one binary rather than one per file: each binary
// links the renderer and every backend, which makes many large ones.

mod support;

mod camera;
mod commands;
mod devices;
mod driver;
mod events;
mod exported;
mod failures;
mod feature_state;
// Rings and surfaces of the backends that report a frame's depth.
#[cfg(any(
    all(feature = "webgpu", not(target_family = "wasm")),
    all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))),
    all(feature = "gl", target_os = "linux"),
))]
mod frame_depth;
mod gestures;
mod placement;
mod resources;
mod sessions;
// Shared targets through an extension of the tests' own.
mod shared_targets;
mod style;
mod surfaces;
mod surfaces_win32;
mod surfaces_window;
mod teardown;
