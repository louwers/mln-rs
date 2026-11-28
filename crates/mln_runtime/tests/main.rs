// The integration tests, in one binary rather than one per file: each binary
// links the renderer and every backend, which makes many large ones.

mod support;

// Where the projection puts places with the camera past the antimeridian.
mod antimeridian;
mod camera;
// The limits on the camera, and the areas that cameras show.
mod camera_bounds;
// The cameras that fit bounds, coordinates and geometries.
mod camera_fit;
mod commands;
// Image sources whose corners make a concave quad.
mod concave_image;
// The padding of cameras, which moves the center that maps draw around.
mod content_insets;
mod devices;
mod driver;
// Tiles that are empty or that their tilesets don't have.
mod empty_tiles;
mod events;
mod exported;
mod failures;
mod feature_state;
// Maps drawn in a viewport flipped upside down.
mod flipped_viewport;
// Text that maps shape with their fallback font faces.
mod font_faces;
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
// Image sources: where they draw, and those without an image yet.
mod image_sources;
// Layers that change after they drew.
mod layer_updates;
// A layer that draws with the backend's own commands, which the frame lends it.
mod lent_commands;
// Maps whose scale factor changes.
mod pixel_ratio;
mod placement;
// The tiles of lower zooms that maps load alongside their zoom's.
mod prefetch;
// Rendered and source queries through filters, global state and zoom.
mod queries;
// Maps that load one style after another.
mod recycle_map;
// The images that layers ask for, and the names that are no image.
mod requested_images;
// Maps whose size changes.
mod resize;
mod resources;
mod sessions;
// Shared targets through an extension of the tests' own.
mod shared_targets;
// The built-in source types' tiles as a host of the runtime sees them load.
mod source_tiles;
// Frames with more clipped tiles than the stencil buffer has values.
mod stencil_overflow;
mod style;
// The style's images, as icons and patterns.
mod style_images;
// Styles from the cache and the network, and what they report and request.
mod style_loading;
mod surfaces;
mod surfaces_win32;
mod surfaces_window;
// Symbol placement and its fades in continuous maps.
mod symbol_placement;
mod teardown;
// Maps over terrain whose DEM tiles are missing or let go of.
mod terrain_dem_tiles;
// Layers draped over terrain.
mod terrain_drape;
// Terrain on the globe.
mod terrain_globe;
// Hillshades over 3D terrain.
mod terrain_hillshade;
// Layers that stand on terrain.
mod terrain_standing;
// The tiles that a map loads for its view, and what it draws meanwhile.
mod tile_loading;
// The tiles that maps load at reduced detail far from the camera.
mod tile_lod;
// The order in which a frame stacks overlapping tiles.
mod tile_order;
// What layers hold and draw as they leave their zoom range.
mod tile_release;
// Tile and glyph requests, and maps whose requests fail.
mod tile_requests;
