// Ports test/gl/bucket.test.cpp (circle, fill, line; on the WebGPU headless
// backend) and test/renderer/fill_bucket.test.cpp.
mod bucket_test;
// Cameras that move faster than the network answers.
mod fast_camera_test;
// Ports test/layout/sort_key.test.cpp.
mod gtest_sky;
mod gtest_subdivision;
mod sort_key_test;
// gtests-data: the test/tile gtests that need the renderer's tiles or
// mln_map's Transform (tests/tile/main.rs lists them).
mod tile;
// gtests-data: the test/text gtests that need mln_renderer (GlyphStore,
// SymbolLayout) or mln_expression's Formatted (tests/text/main.rs lists them).
mod text;
// gtests-core: test/renderer/image_manager.test.cpp.
mod gtest_image_store;
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
// Port-only: the renderer frees what it lets go of on its workers.
mod drop_on_workers_test;
// Port-only: malformed vector tiles become tile errors, not panics.
mod malformed_tile_test;
// Port-only: a tile answers its worker's dependency requests from the
// renderer's stores.
mod tile_dependencies_test;
// Port-only: a tile's layout in two stages, and the worker over them.
mod tile_layout_test;
// The VectorTile tests of test/tile/vector_tile.test.cpp.
mod vector_tile_test;

/// Port-only: the renderer's glyph and image stores and atlas, which answer
/// the dependency requests of the tiles under test as the renderer's update
/// does.
pub(crate) struct TileDependencyStores {
    pub(crate) glyphs: crate::text::glyph_store::GlyphStore,
    pub(crate) images: crate::image_store::ImageStore,
    pub(crate) atlas: crate::gfx::dynamic_texture_atlas::DynamicTextureAtlas,
}

impl TileDependencyStores {
    /// Stores whose glyph responses arrive on `run_loop`, with no images and
    /// no sprite loaded.
    pub(crate) fn new(run_loop: &mln_actor::RunLoop) -> Self {
        TileDependencyStores {
            glyphs: crate::text::glyph_store::GlyphStore::new(
                None,
                std::sync::Arc::default(),
                crate::text::font_face_engine::default_engine(),
                run_loop.clone(),
                std::rc::Rc::new(|| {}),
            ),
            images: crate::image_store::ImageStore::default(),
            atlas: crate::gfx::dynamic_texture_atlas::DynamicTextureAtlas::new(),
        }
    }

    /// Starts an update, as the renderer's does: takes in the glyph
    /// responses that arrived, and gives the images asked for their update.
    pub(crate) fn update(&mut self) {
        self.glyphs.update();
        self.images.begin_update();
    }

    /// Answers `tile`'s dependency request, if the stores can; returns
    /// whether they did.
    pub(crate) fn resolve(&mut self, tile: &crate::tile::geometry_tile::GeometryTile) -> bool {
        tile.inner().resolve_dependencies(&mut crate::tile::geometry_tile::DependencyStores {
            glyphs: &mut self.glyphs,
            images: &mut self.images,
            atlas: &mut self.atlas,
        })
    }

    /// Starts an update and answers the requests of `source`'s tiles.
    pub(crate) fn resolve_source(&mut self, source: &dyn crate::render_source::RenderSource) {
        self.update();
        source.for_each_geometry_tile(&mut |tile| {
            self.resolve(tile);
        });
    }
}
