// The integration tests, in one binary rather than one per file: each binary
// links the renderer and every backend, which makes many large ones.

mod support;

// An image source whose corners make a concave quad draws inside its outline.
mod concave_image_test;
// Empty and absent tiles of vector and raster sources through Map -> HeadlessFrontend.
mod empty_tile_test;
// A viewport flipped upside down draws the map upside down.
mod flipped_viewport_test;
// Frames that fail on the GPU: stills that end with the error, and
// continuous maps that report it once and draw again later.
mod gpu_failure_test;
// gtests-core: test/api/{annotations,api_misuse,custom_geometry_source,
// custom_layer}.test.cpp and test/api/recycle_map.cpp.
mod gtest_api;
// gtests-core: test/map/map.test.cpp through Map (over a
// StubFileSource/FakeFileSource) -> HeadlessFrontend.
mod gtest_map;
// gtests-core: test/util/memory.test.cpp (Map over a StubFileSource with the
// test/fixtures/resources styles).
mod gtest_memory;
// gtests-core: test/map/prefetch.test.cpp.
mod gtest_prefetch;
// gtests-core: test/api/query.test.cpp.
mod gtest_query;
// The built-in source types' tiles as a host of the runtime sees them load.
mod source_tiles;
// gtests-core: test/renderer/symbol_rendering.test.cpp (maplibre-native #4628).
mod gtest_symbol_rendering;
mod gtest_tile_lod;
// A hillshade's shading of a plane slope.
mod hillshade_test;
// A layer that draws with the backend's own commands, which the frame lends it.
mod lent_commands_test;
// gtests-data: test/text/local_glyph_rasterizer.test.cpp (renders a map
// through HeadlessFrontend with a StubFileSource).
mod local_glyph_rasterizer_test;
mod map_snapshot_test;
// Symbol placement in a continuous map runs again when, and only when,
// what it places from changes.
mod placement_test;
// Map::set_pixel_ratio and HeadlessFrontend::set_pixel_ratio.
mod pixel_ratio_test;
// The textures that raster, hillshade and color-relief layers keep of their tiles.
mod raster_textures_test;
// Map::set_size and HeadlessFrontend::set_size with a heatmap, which draws into a texture of its own.
mod resize_test;
// Frontends that share one device, each drawing its own map.
mod shared_device_test;
// A frame whose stencil values overflow, with several world copies in view.
mod stencil_overflow_test;
// A file source that answers inside `load`.
mod sync_file_source_test;
// Text that the device's fonts shape, with a font file standing in for them.
mod system_fonts_test;
// The layers that drape over terrain, drawn through the terrain's drapes.
mod terrain_drape_test;
// A hillshade over terrain lit from the viewport, which the surface shades.
mod terrain_hillshade_test;
// Vertical perspective close in draws what Mercator draws.
mod globe_precision_test;
// Terrain on the globe, draped as on Mercator.
mod terrain_globe_test;
// The layers that stand on terrain, and hide behind it.
mod terrain_standing_test;
// Tiles showing each answer to their open requests, as their data expires.
mod tile_expiry_test;
// Features of two tiles that overlap stack in the frame's order of its tiles.
mod tile_order_test;
