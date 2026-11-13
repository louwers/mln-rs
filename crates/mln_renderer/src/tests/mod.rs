// Ports test/gl/bucket.test.cpp (circle, fill, line; on the WebGPU headless
// backend) and test/renderer/fill_bucket.test.cpp.
mod bucket_test;
// Ports test/layout/sort_key.test.cpp.
mod gtest_sky;
mod gtest_subdivision;
mod sort_key_test;
// gtests-data: the test/tile gtests that need the renderer's tiles or
// mln_map's Transform (tests/tile/main.rs lists them).
mod tile;
// gtests-data: the test/text gtests that need mln_renderer (GlyphManager,
// SymbolLayout) or mln_expression's Formatted (tests/text/main.rs lists them).
mod text;
// gtests-core: test/renderer/image_manager.test.cpp.
mod gtest_image_manager;
// gtests-core: test/renderer/dynamic_texture_atlas.test.cpp.
mod gtest_dynamic_texture_atlas;
mod gtest_fill_generator;
// gtests-core: test/renderer/pattern_atlas.test.cpp.
mod gtest_fill_large_mesh_arrays;
mod gtest_pattern_atlas;
mod gtest_tile_lod;
// gtests-core: test/algorithm/update_renderables.test.cpp.
mod gtest_update_renderables;
// gtests-core: test/geometry/line_atlas.test.cpp.
mod gtest_line_atlas;
// gtests-core: test/renderer/style_diff.test.cpp.
mod gtest_style_diff;
// Ports test/util/tile_range.test.cpp (marker `// gtest: Group.Name`).
mod gtest_tile_range;
// test/util/tile_cover.test.cpp: needs mln_map's Transform, which is above
// this crate, so it is an integration test rather than a unit test.
mod tile_cover_test;
// test/tile/geojson_tile.test.cpp and the GeoJSON parts of
// test/style/source.test.cpp: need mln_map's Transform (above this crate).
mod geojson_tile_test;
mod source_test;
// Port-only: malformed vector tiles become tile errors, not panics.
mod malformed_tile_test;
// The VectorTile tests of test/tile/vector_tile.test.cpp.
mod vector_tile_test;
