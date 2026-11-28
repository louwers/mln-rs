// The integration tests, in one binary rather than one per file: each binary
// links the renderer and every backend, which makes many large ones.

mod support;

mod camera;
mod commands;
// Image sources whose corners make a concave quad.
mod concave_image;
mod devices;
mod driver;
mod events;
mod exported;
mod failures;
mod feature_state;
// Maps drawn in a viewport flipped upside down.
mod flipped_viewport;
// Rings and surfaces of the backends that report a frame's depth.
#[cfg(any(
    all(feature = "webgpu", not(target_family = "wasm")),
    all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))),
    all(feature = "gl", target_os = "linux"),
))]
mod frame_depth;
mod gestures;
// Vertical perspective close in, which draws as Mercator does.
mod globe_precision;
// Hillshades over DEM tiles that the tests make.
mod hillshade;
// Maps whose scale factor changes.
mod pixel_ratio;
mod placement;
// Maps whose size changes.
mod resize;
mod resources;
mod sessions;
// Shared targets through an extension of the tests' own.
mod shared_targets;
// Frames with more clipped tiles than the stencil buffer has values.
mod stencil_overflow;
mod style;
mod surfaces;
mod surfaces_win32;
mod surfaces_window;
mod teardown;
// Layers draped over terrain.
mod terrain_drape;
// Terrain on the globe.
mod terrain_globe;
// Hillshades over 3D terrain.
mod terrain_hillshade;
// Layers that stand on terrain.
mod terrain_standing;
// The order in which a frame stacks overlapping tiles.
mod tile_order;
