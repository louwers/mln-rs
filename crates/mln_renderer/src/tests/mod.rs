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
// Port-only: a tile drives its layout jobs.
mod tile_jobs_test;
// Port-only: a tile's layout in two stages, and the worker over them.
mod tile_layout_test;
// The VectorTile tests of test/tile/vector_tile.test.cpp.
mod vector_tile_test;

/// Port-only: drives the geometry tiles under test as the renderer's update
/// does: their jobs run on `jobs`' workers, and the renderer's glyph and
/// image stores and atlas resolve what the jobs need.
pub(crate) struct TileDriver {
    pub(crate) glyphs: crate::text::glyph_store::GlyphStore,
    pub(crate) images: crate::image_store::ImageStore,
    pub(crate) atlas: crate::gfx::dynamic_texture_atlas::DynamicTextureAtlas,
    pub(crate) jobs: crate::tile::tile_workers::TileJobs,
}

impl TileDriver {
    /// A driver whose jobs run on `workers`, whose glyph responses arrive on
    /// `run_loop`, with no images and no sprite loaded.
    pub(crate) fn new(run_loop: &mln_actor::RunLoop, workers: Box<dyn crate::tile::tile_workers::TileWorkers>) -> Self {
        TileDriver {
            glyphs: crate::text::glyph_store::GlyphStore::new(
                None,
                std::sync::Arc::default(),
                crate::text::font_face_engine::default_engine(),
                run_loop.clone(),
                std::rc::Rc::new(|| {}),
            ),
            images: crate::image_store::ImageStore::default(),
            atlas: crate::gfx::dynamic_texture_atlas::DynamicTextureAtlas::new(),
            jobs: crate::tile::tile_workers::TileJobs::new(workers),
        }
    }

    /// A driver whose jobs run inside their submission.
    pub(crate) fn inline(run_loop: &mln_actor::RunLoop) -> Self {
        Self::new(run_loop, Box::new(crate::tile::tile_workers::InlineWorkers::default()))
    }

    /// A driver whose jobs run on `thread_pool`.
    pub(crate) fn pool(run_loop: &mln_actor::RunLoop, thread_pool: &mln_actor::TaggedScheduler) -> Self {
        let workers = crate::tile::tile_workers::PoolWorkers::new(thread_pool.clone(), 8, std::sync::Arc::new(|| {}));
        Self::new(run_loop, Box::new(workers))
    }

    /// One update of `tiles`, as the renderer's: takes in the glyph
    /// responses, gives the images asked for their update, applies the
    /// replies that arrived, resolves what the jobs wait for and submits the
    /// next jobs. Returns whether a tile shows a new layout.
    pub(crate) fn update(&mut self, tiles: &[&crate::tile::geometry_tile::GeometryTile]) -> bool {
        self.glyphs.update();
        self.images.begin_update();
        self.jobs.receive();
        let mut laid_out = false;
        for tile in tiles {
            laid_out |= tile.inner().take_reply(&mut self.jobs);
        }
        drop(self.jobs.end_receive());
        let mut stores = crate::tile::geometry_tile::DependencyStores {
            glyphs: &mut self.glyphs,
            images: &mut self.images,
            atlas: &mut self.atlas,
        };
        for tile in tiles {
            laid_out |= tile.inner().drive(&mut self.jobs, &mut stores);
        }
        laid_out
    }

    /// One update of `tile`, a geometry tile.
    pub(crate) fn update_tile(&mut self, tile: &dyn crate::tile::tile::Tile) -> bool {
        self.update(&[tile.as_geometry_tile().expect("a geometry tile")])
    }

    /// One update of `source`'s tiles.
    pub(crate) fn update_source(&mut self, source: &dyn crate::render_source::RenderSource) -> bool {
        let mut tiles = Vec::new();
        source.for_each_geometry_tile(&mut |tile| tiles.push(tile));
        self.update(&tiles)
    }
}

/// Port-only: a source's style of `layers`, without global state or font
/// faces.
pub(crate) fn style(
    layers: &[std::sync::Arc<dyn mln_style::layer_properties::LayerProperties>],
) -> std::sync::Arc<crate::tile::job::SourceStyle> {
    std::sync::Arc::new(crate::tile::job::SourceStyle {
        version: 0,
        layers: layers.to_vec(),
        global_state: None,
        font_faces: None,
    })
}

/// Port-only: the counters of a source under test.
pub(crate) fn test_source() -> std::rc::Rc<crate::tile::tile_workers::SourceCounters> {
    std::rc::Rc::new(crate::tile::tile_workers::SourceCounters::new(crate::tile::job::SourceKey(1)))
}
