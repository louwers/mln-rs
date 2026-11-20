// The integration tests, in one binary rather than one per file: each binary
// links the renderer and every backend, which makes many large ones.

mod support;

mod custom_layer_test;
// Map::set_debug's tile overlays and the rendering stats view.
mod debug_options_test;
// Empty and absent tiles of vector and raster sources through Map -> HeadlessFrontend.
mod empty_tile_test;
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
// gtests-core: test/renderer/symbol_rendering.test.cpp (maplibre-native #4628).
mod gtest_symbol_rendering;
mod gtest_tile_lod;
// gtests-data: test/text/local_glyph_rasterizer.test.cpp (renders a map
// through HeadlessFrontend with a StubFileSource).
mod local_glyph_rasterizer_test;
// The M1 background render tests through Map -> HeadlessFrontend, without the
// render-test runner (see the file comment).
mod m1_background_render_test;
mod map_snapshot_test;
// Symbol placement in a continuous map runs again when, and only when,
// what it places from changes.
mod placement_test;
// Map::set_pixel_ratio and HeadlessFrontend::set_pixel_ratio.
mod pixel_ratio_test;
// Map::set_size and HeadlessFrontend::set_size with a heatmap, which draws into a texture of its own.
mod resize_test;
// Text that the device's fonts shape, with a font file standing in for them.
mod system_fonts_test;
// Tiles reloading as their data expires, over an online file source.
mod tile_expiry_test;
