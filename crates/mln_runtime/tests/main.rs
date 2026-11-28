// The integration tests, in one binary rather than one per file: each binary
// links the renderer and every backend, which makes many large ones.

mod support;

mod camera;
// The limits on the camera, and the areas that cameras show.
mod camera_bounds;
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
// The style's global state, as layers draw it.
mod global_state;
// Maps drawn as a globe: their tiles and symbols.
mod globe;
// Vertical perspective close in, which draws as Mercator does.
mod globe_precision;
// Hillshades over DEM tiles that the tests make.
mod hillshade;
// Layers that change after they drew.
mod layer_updates;
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
// The style's images, as icons and patterns.
mod style_images;
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
// What layers hold and draw as they leave their zoom range.
mod tile_release;
// Tile and glyph requests, and maps whose requests fail.
mod tile_requests;
