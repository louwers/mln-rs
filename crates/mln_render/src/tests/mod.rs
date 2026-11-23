// `dot`, `tint`, `shade` and `mark`, the tests' layer types.
pub(crate) mod fixture_layer;
// `scripted`, `scripted-raster` and `scripted-dem`, the tests' source types,
// whose tiles load what the test sends them.
pub(crate) mod scripted_origin;
// Ports test/gl/bucket.test.cpp (raster; on the WebGPU headless backend).
// mln_layers' tests port the circle, fill, line and symbol parts.
mod bucket_test;
// Cameras that move faster than the network answers.
mod fast_camera_test;
mod gtest_sky;
mod gtest_subdivision;
// The surfaces that tiles' images drape over: the globe's grids of masked
// tiles, and the quads of masks.
mod image_surface_test;
// Raster, DEM and custom geometry tiles, and tile coordinates.
mod tile;
// The glyph store and cache, and formatted text.
mod text;
// The image store: images, their requests and missing-image events.
mod gtest_image_store;
// The dynamic texture atlas's sharing and packing of images and glyphs.
mod gtest_dynamic_texture_atlas;
// The tile cover's choice of ideal, parent and child tiles.
mod gtest_update_renderables;
// Ports test/util/tile_range.test.cpp (marker `// gtest: Group.Name`).
mod gtest_tile_range;
// test/tile/geojson_tile.test.cpp, over features the test gives the tiles.
mod geojson_tile_test;
// test/style/source.test.cpp's cases of the tile cover, over scripted
// sources; mln_sources has the built-in source types' cases. Needs the map's
// Transform (mln_runtime, above this crate).
mod source_test;
// Port-only: the renderer frees what it lets go of on its workers.
mod drop_on_workers_test;

mod programs_test;
// The context destroys what it released after what was made of it.
mod release_test;
// Port-only: a tile drives its layout jobs.
mod tile_jobs_test;
// Port-only: a tile's layout in two stages, and the worker over them.
mod tile_layout_test;
// Port-only: the tile store keeps a tile's data, work and contents apart.
mod tile_store_test;
// The VectorTile tests of test/tile/vector_tile.test.cpp.
mod vector_tile_test;

/// A pool for a test's background work.
pub(crate) fn pool() -> mln_util::pool::Pool {
    mln_util::pool::Pool::new(4)
}

pub(crate) use crate::testing::{tile_store, update};

/// Port-only: a tile store whose jobs run on `pool`.
pub(crate) fn pool_store(pool: &mln_util::pool::TaskGroup) -> crate::tile::store::TileStore {
    tile_store(pool, Box::new(crate::tile::tile_workers::PoolWorkers::new(pool.clone(), 8)))
}

/// Port-only: one tile of a source of its own in a store, driven as the
/// renderer drives it, whose data the test gives it.
pub(crate) struct OneTile {
    pub(crate) store: crate::tile::store::TileStore,
    pub(crate) key: crate::tile::job::SourceKey,
    pub(crate) id: mln_tile_id::OverscaledTileID,
    /// What the source's tiles load.
    pub(crate) script: scripted_origin::Script,
    mode: mln_map_types::MapMode,
}

impl OneTile {
    /// Tile `id` of kind `kind` in `store`, laid out for `mode`.
    pub(crate) fn new(
        mut store: crate::tile::store::TileStore,
        kind: mln_style::layer_type::SourceKind,
        id: mln_tile_id::OverscaledTileID,
        mode: mln_map_types::MapMode,
    ) -> Self {
        let script = scripted_origin::Script::default();
        let key = store.add_source("s".to_owned(), script.origin(kind));
        let mut tiles = OneTile { store, key, id, script, mode };
        tiles.add(id);
        tiles
    }

    fn params(&self) -> crate::layout::tile_layout::LayoutParams {
        crate::layout::tile_layout::LayoutParams {
            mode: self.mode,
            pixel_ratio: 1.0,
            show_collision_boxes: false,
            cpu_rasterizer: false,
            subdivision_granularity: mln_util::subdivision_granularity::SubdivisionGranularitySetting::none(),
            first_load: true,
        }
    }

    /// Tiles `ids` of a source of `kind` whose data `script` gives, in
    /// `store`, laid out for `mode`. Their loads start as a cover starts
    /// them.
    pub(crate) fn with_script(
        mut store: crate::tile::store::TileStore,
        kind: mln_style::layer_type::SourceKind,
        script: &scripted_origin::Script,
        ids: &[mln_tile_id::OverscaledTileID],
        mode: mln_map_types::MapMode,
    ) -> Self {
        let key = store.add_source("source".to_owned(), script.origin(kind));
        let mut tiles = OneTile { store, key, id: ids[0], script: script.clone(), mode };
        let params = tiles.params();
        crate::testing::seed(&mut tiles.store, key, ids, params);
        tiles
    }

    /// Sets how much tile `id` is needed.
    pub(crate) fn set_necessity(
        &mut self,
        id: &mln_tile_id::OverscaledTileID,
        necessity: crate::tile::tile::TileNecessity,
    ) {
        self.with_at(id, |tile| tile.set_necessity(necessity));
    }

    /// How many changes the store reported since the last call, after it
    /// took in what arrived.
    pub(crate) fn changes(&mut self) -> usize {
        self.store.receive();
        self.store
            .take_events()
            .into_iter()
            .filter(|event| matches!(event, crate::tile::store::StoreEvent::Changed))
            .count()
    }

    /// Adds another tile, `id`, to the source, which loads as the script
    /// says once it is required.
    pub(crate) fn add(&mut self, id: mln_tile_id::OverscaledTileID) {
        let params = self.params();
        crate::testing::seed(&mut self.store, self.key, &[id], params);
    }

    /// Adds another tile, `id`, to the source, which starts from the same
    /// tile in another world copy if one is renderable, as a flat map's
    /// cover starts it.
    pub(crate) fn add_from_copy(&mut self, id: mln_tile_id::OverscaledTileID) {
        let params = self.params();
        let (source, ctx) = self.store.source_mut(self.key).expect("the source");
        source.add_tile(id, params, true, ctx);
    }

    pub(crate) fn tile(&self) -> &crate::tile::tile::Tile {
        self.tile_at(&self.id)
    }

    /// The layout parameters of the tile.
    pub(crate) fn tile_params(&self) -> crate::layout::tile_layout::LayoutParams {
        self.layout().params
    }

    /// The layout of the tile, a geometry tile.
    pub(crate) fn layout(&self) -> &crate::tile::pipeline::Layout {
        self.tile().pipeline.layout().expect("a geometry tile")
    }

    pub(crate) fn tile_at(&self, id: &mln_tile_id::OverscaledTileID) -> &crate::tile::tile::Tile {
        self.store.source(self.key).and_then(|source| source.tile(id)).expect("the tile")
    }

    /// Changes the tile through `f`.
    pub(crate) fn with<R>(&mut self, f: impl FnOnce(&mut crate::tile::tile_mut::TileMut<'_>) -> R) -> R {
        let id = self.id;
        self.with_at(&id, f)
    }

    /// Changes tile `id` through `f`.
    pub(crate) fn with_at<R>(
        &mut self,
        id: &mln_tile_id::OverscaledTileID,
        f: impl FnOnce(&mut crate::tile::tile_mut::TileMut<'_>) -> R,
    ) -> R {
        let (source, ctx) = self.store.source_mut(self.key).expect("the source");
        source.update(id, ctx, f).expect("the tile")
    }

    pub(crate) fn set_style(&mut self, style: &std::sync::Arc<crate::tile::job::SourceStyle>) {
        let params = self.params();
        self.with(|tile| tile.set_style(style, params.pixel_ratio, params.subdivision_granularity));
    }

    /// Gives a geometry tile `data`, as a load of its own.
    pub(crate) fn set_data(&mut self, data: Option<mln_tile::TileData>) {
        self.with(|tile| tile.take_data(data, false));
    }

    /// Gives raster or raster-dem tile `id` the image `data`, as a load of
    /// its own; a DEM's elevations are in the Mapbox encoding.
    pub(crate) fn set_image(&mut self, id: &mln_tile_id::OverscaledTileID, data: Option<mln_storage::Data>) {
        let dem =
            self.store.source(self.key).expect("the source").info.kind == mln_style::layer_type::SourceKind::RasterDem;
        let encoding = dem.then_some(mln_util::tileset::RasterEncoding::Mapbox);
        self.with_at(id, |tile| tile.take_image(data.map(|data| (data, encoding)), false));
    }

    /// Gives tile `id` no data, as a load that finds the tileset has no
    /// tile there.
    pub(crate) fn set_missing(&mut self, id: &mln_tile_id::OverscaledTileID) {
        #[expect(clippy::redundant_closure_for_method_calls, reason = "`with_at` takes a tile of any lifetime")]
        self.with_at(id, |tile| tile.take_missing());
    }

    /// What the store reported its tiles went through since the last call.
    pub(crate) fn actions(&mut self) -> Vec<crate::tile::tile::TileOperation> {
        self.store
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                crate::tile::store::StoreEvent::Action { op, .. } => Some(op),
                crate::tile::store::StoreEvent::Changed | crate::tile::store::StoreEvent::Error { .. } => None,
            })
            .collect()
    }

    /// The errors the store reported since the last call.
    pub(crate) fn errors(&mut self) -> Vec<String> {
        self.store
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                crate::tile::store::StoreEvent::Error { error, .. } => Some(error.to_string()),
                crate::tile::store::StoreEvent::Changed | crate::tile::store::StoreEvent::Action { .. } => None,
            })
            .collect()
    }

    /// One update of the store.
    pub(crate) fn update(&mut self) -> bool {
        update(&mut self.store)
    }

    /// Updates the store until the tiles `ids` are all complete, updating at
    /// least once.
    ///
    /// # Panics
    ///
    /// If they don't complete within 30 seconds.
    pub(crate) fn settle(&mut self, ids: &[mln_tile_id::OverscaledTileID]) {
        let deadline = mln_util::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            self.update();
            if ids.iter().all(|id| self.tile_at(id).is_complete()) {
                return;
            }
            assert!(mln_util::time::Instant::now() < deadline, "the tiles didn't complete");
            std::thread::yield_now();
        }
    }

    /// Whether the tile shows a layout.
    pub(crate) fn shows_layout(&self) -> bool {
        self.tile().contents().and_then(crate::tile::contents::Contents::geometry).is_some()
    }

    /// Lets go of the tile, as its source does when it leaves the cache.
    pub(crate) fn drop_tile(&mut self) {
        let id = self.id;
        self.drop_tile_at(&id);
    }

    /// Lets go of tile `id`.
    pub(crate) fn drop_tile_at(&mut self, id: &mln_tile_id::OverscaledTileID) {
        let (source, ctx) = self.store.source_mut(self.key).expect("the source");
        source.release(id, ctx);
    }
}

/// Port-only: a source's style of `layers`, without global state or font
/// faces.
pub(crate) fn style(
    layers: &[std::sync::Arc<dyn crate::erased_layer::ErasedEvaluatedLayer>],
) -> std::sync::Arc<crate::tile::job::SourceStyle> {
    std::sync::Arc::new(crate::tile::job::SourceStyle {
        version: 0,
        layers: layers.to_vec(),
        global_state: None,
        font_faces: None,
        glyph_pbfs: true,
    })
}

/// Port-only: the render layer of the layer `style`, of a built-in type.
pub(crate) fn render_layer(
    style: &std::sync::Arc<dyn mln_style::layer_type::ErasedLayerStyle>,
) -> Box<dyn crate::render_layer::RenderLayer> {
    let registry = crate::Registry::with_builtins();
    let layer_type = registry.layer_type(style.info().name).expect("a built-in layer type");
    layer_type.create_render_layer(style, &mut crate::gfx::context::Resources::detached()).expect("a layer of its type")
}

/// The render source of `source`, through the tests' registry.
pub(crate) fn create_render_source(
    source: &mln_style::Source,
    store: &mut crate::tile::store::TileStore,
) -> Box<dyn crate::render_source::RenderSource> {
    crate::render_source::create(&crate::registry::Registry::with_builtins(), source, store)
        .expect("a source type of the tests")
}

/// Runs `f` with the `Gpu` of a headless WebGPU backend, which compiles the
/// engine's programs, and a context over it, which it then tears down.
pub(crate) fn with_context(f: impl FnOnce(&mut dyn mln_gfx::gpu::Gpu, &mut crate::gfx::context::Resources)) {
    use mln_gfx::RendererBackend as _;
    use mln_gfx::gpu::Gpu as _;
    let device = mln_webgpu::Device::new(None).expect("device");
    let mut backend = mln_webgpu::RendererBackend::new(&device);
    let gpu = backend.gpu();
    let mut context = crate::gfx::context::Resources::new(*gpu.caps(), 1.0);
    f(gpu, &mut context);
    context.tear_down(gpu);
}
