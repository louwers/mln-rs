// Ports test/gl/bucket.test.cpp (circle, fill, line; on the WebGPU headless
// backend) and test/renderer/fill_bucket.test.cpp.
mod bucket_test;
// Cameras that move faster than the network answers.
mod fast_camera_test;
mod fixture_log;
// Ports test/layout/sort_key.test.cpp.
mod gtest_sky;
mod gtest_subdivision;
mod sort_key_test;
// gtests-data: the test/tile gtests that need the renderer's tiles or
// mln_map's Transform (tests/tile/main.rs lists them).
mod tile;
// gtests-data: the test/text gtests that need mln_renderer (GlyphStore,
// SymbolLayout) or mln_style's Formatted (tests/text/main.rs lists them).
mod text;
// gtests-core: test/renderer/image_manager.test.cpp.
mod gtest_image_store;
// gtests-core: test/renderer/dynamic_texture_atlas.test.cpp.
mod gtest_dynamic_texture_atlas;
mod gtest_fill_mesh;
// gtests-core: test/renderer/pattern_atlas.test.cpp.
mod gtest_pattern_atlas;
mod gtest_tile_lod;
// gtests-core: test/algorithm/update_renderables.test.cpp.
mod gtest_update_renderables;
// gtests-core: test/geometry/line_atlas.test.cpp.
mod gtest_line_atlas;
// Ports test/util/tile_range.test.cpp (marker `// gtest: Group.Name`).
mod gtest_tile_range;
// test/util/tile_cover.test.cpp: needs mln_map's Transform, which is above
// this crate, so it is an integration test rather than a unit test.
mod tile_cover_test;
// test/tile/geojson_tile.test.cpp and the GeoJSON parts of
// test/style/source.test.cpp: need mln_map's Transform (above this crate).
mod geojson_tile_test;
mod source_test;
// The terrain's drapes: their projection, their stencil, the tiles a drape
// draws, what draws one again, and their byte cap.
mod drape_reuse_test;
mod drape_test;
// Port-only: the renderer frees what it lets go of on its workers.
mod drop_on_workers_test;
// Port-only: malformed vector tiles become tile errors, not panics.
mod malformed_tile_test;
// The context destroys what it released after what was made of it.
mod release_test;
// Port-only: a tile drives its layout jobs.
mod tile_jobs_test;
// Port-only: a tile's layout in two stages, and the worker over them.
mod tile_layout_test;
// Port-only: the tile store keeps a tile's data, work and contents apart.
mod tile_store_test;
// Port-only: whether terrain draws, and its cover.
mod terrain_test;
// The VectorTile tests of test/tile/vector_tile.test.cpp.
mod vector_tile_test;

/// A pool for a test's background work.
pub(crate) fn pool() -> mln_util::pool::Pool {
    mln_util::pool::Pool::new(4)
}

/// Port-only: a tile store whose jobs run on `workers` and which frees and
/// parses glyphs on `pool`, with no images and no sprite loaded.
pub(crate) fn tile_store(
    pool: &mln_util::pool::TaskGroup,
    workers: Box<dyn crate::tile::tile_workers::TileWorkers>,
) -> crate::tile::store::TileStore {
    let glyphs = crate::text::glyph_store::GlyphStore::new(
        mln_text::local_fonts::LocalGlyphs::default(),
        std::sync::Arc::default(),
        pool.clone(),
        crate::text::font_face_engine::default_engine(),
        std::sync::Arc::new(|| {}),
    );
    crate::tile::store::TileStore::new(
        workers,
        glyphs,
        crate::image_store::ImageStore::default(),
        pool.clone(),
        std::sync::Arc::new(|| {}),
    )
}

/// Port-only: a tile store whose jobs run on `pool`.
pub(crate) fn pool_store(pool: &mln_util::pool::TaskGroup) -> crate::tile::store::TileStore {
    tile_store(pool, Box::new(crate::tile::tile_workers::PoolWorkers::new(pool.clone(), 8)))
}

/// Port-only: one update of `store`'s tiles, as the renderer's: gives the
/// images asked for their update, takes in what arrived and starts the next
/// jobs. Returns whether a tile of a source with synchronous updates laid
/// out.
pub(crate) fn update(store: &mut crate::tile::store::TileStore) -> bool {
    store.images_mut().begin_update();
    store.receive();
    store.drive()
}

/// Port-only: one tile of a source of its own in a store, driven as the
/// renderer drives it, whose data the test gives it.
pub(crate) struct OneTile {
    pub(crate) store: crate::tile::store::TileStore,
    pub(crate) key: crate::tile::job::SourceKey,
    pub(crate) id: mln_tile_id::OverscaledTileID,
    mode: mln_map_types::MapMode,
}

impl OneTile {
    /// Tile `id` of kind `kind` in `store`, laid out for `mode`.
    pub(crate) fn new(
        mut store: crate::tile::store::TileStore,
        kind: crate::tile::tile::TileKind,
        id: mln_tile_id::OverscaledTileID,
        mode: mln_map_types::MapMode,
    ) -> Self {
        let key = store.add_source("s".to_owned(), kind, false);
        let mut tiles = OneTile { store, key, id, mode };
        tiles.add(id);
        tiles
    }

    fn params(&self) -> crate::layout::tile_layout::LayoutParams {
        crate::layout::tile_layout::LayoutParams {
            mode: self.mode,
            pixel_ratio: 1.0,
            subdivision_granularity: mln_style::subdivision_granularity::SubdivisionGranularitySetting::none(),
            first_load: true,
        }
    }

    /// Tiles `ids` of a source of `kind` whose data comes from `origin`, in
    /// `store`, laid out for `mode`; `single_layer` for a source whose data
    /// has one layer. Their loads start as a cover starts them.
    pub(crate) fn with_origin(
        mut store: crate::tile::store::TileStore,
        kind: crate::tile::tile::TileKind,
        single_layer: bool,
        origin: crate::tile::loading::DataOrigin,
        ids: &[mln_tile_id::OverscaledTileID],
        mode: mln_map_types::MapMode,
    ) -> Self {
        let key = store.add_source("source".to_owned(), kind, single_layer);
        let mut tiles = OneTile { store, key, id: ids[0], mode };
        let params = tiles.params();
        let (source, ctx) = tiles.store.source_mut(key).expect("the source");
        source.info.origin = Some(origin);
        for id in ids {
            let tile = source.info.new_tile(*id, params).expect("a tile");
            source.insert(tile);
            source.update(id, ctx, |tile| {
                tile.start_load();
            });
        }
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

    /// Moves tile `from` to id `to`, as a wrap jump does.
    pub(crate) fn rekey(&mut self, from: &mln_tile_id::OverscaledTileID, to: mln_tile_id::OverscaledTileID) {
        let (source, ctx) = self.store.source_mut(self.key).expect("the source");
        assert!(source.tile(from).is_some(), "the tile");
        source.rekey(from, to, ctx);
        if self.id == *from {
            self.id = to;
        }
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

    /// Adds another tile, `id`, to the source, which loads nothing itself.
    pub(crate) fn add(&mut self, id: mln_tile_id::OverscaledTileID) {
        let params = self.params();
        let (source, _) = self.store.source_mut(self.key).expect("the source");
        let generation = source.info.next();
        let load = crate::tile::loading::Load {
            necessity: crate::tile::tile::TileNecessity::Optional,
            origin: crate::tile::loading::OriginLoad::GeoJson { load: None },
        };
        let pipeline = crate::tile::pipeline::Pipeline::new(source.info.kind, generation, params);
        let tile = crate::tile::tile::Tile::new(id, load, pipeline);
        source.insert(tile);
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
        self.with(|tile| tile.take_input(crate::tile::tile::TileInput::Geometry { data, missing: false }));
    }

    /// Gives raster tile `id` the image `data`, as a load of its own.
    pub(crate) fn set_image(&mut self, id: &mln_tile_id::OverscaledTileID, data: Option<mln_storage::Data>) {
        self.with_at(id, |tile| tile.take_input(crate::tile::tile::TileInput::Image { data, missing: false }));
    }

    /// Gives tile `id` no data, as a load that finds the tileset has no
    /// tile there.
    pub(crate) fn set_missing(&mut self, id: &mln_tile_id::OverscaledTileID) {
        let input = match self.store.source(self.key).expect("the source").info.kind {
            crate::tile::tile::TileKind::Geometry => {
                crate::tile::tile::TileInput::Geometry { data: None, missing: true }
            }
            crate::tile::tile::TileKind::Raster | crate::tile::tile::TileKind::Dem(_) => {
                crate::tile::tile::TileInput::Image { data: None, missing: true }
            }
        };
        self.with_at(id, |tile| tile.take_input(input));
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
    layers: &[std::sync::Arc<dyn crate::layer_type::ErasedEvaluatedLayer>],
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
    style: &std::sync::Arc<dyn mln_style::layer::ErasedLayerStyle>,
) -> Box<dyn crate::render_layer::RenderLayer> {
    let registry = crate::Registry::with_builtins();
    let layer_type = registry.layer_type(style.info().name).expect("a built-in layer type");
    layer_type.create_render_layer(style).expect("a layer of its type")
}

/// The render source of `source`, through the built-in registry.
pub(crate) fn create_render_source(
    source: &mln_style::Source,
    store: &mut crate::tile::store::TileStore,
) -> Box<dyn crate::render_source::RenderSource> {
    crate::render_source::create(&crate::registry::Registry::with_builtins(), source, store)
        .expect("a built-in source type")
}

/// `source` with the description that its type knows without a fetch, such
/// as the tileset a style lists, as the map's loader gives it.
pub(crate) fn ready(source: mln_style::Source) -> mln_style::Source {
    match source.data().plan_load() {
        mln_style::source::Load::Ready(description) => {
            source.with_description(Some(description)).expect("a description of the source's type")
        }
        _ => source,
    }
}

/// A WebGPU device, or `None` where no adapter is available: the test is
/// skipped there, unless `MLN_RENDER_BACKEND` asks for `webgpu`.
pub(super) fn webgpu_device() -> Option<mln_webgpu::Device> {
    let requested = std::env::var("MLN_RENDER_BACKEND").is_ok_and(|backend| backend.eq_ignore_ascii_case("webgpu"));
    match mln_webgpu::Device::new(None) {
        Ok(device) => Some(device),
        Err(error) if error.kind == mln_gfx::ErrorKind::Unsupported && !requested => {
            eprintln!("skipped: {error}");
            None
        }
        Err(error) => panic!("WebGPU device: {error}"),
    }
}

/// Runs `f` with the `Gpu` of a headless WebGPU backend and a context over
/// it, which it then tears down. Skipped where no WebGPU adapter is
/// available unless `MLN_RENDER_BACKEND` asks for `webgpu`.
fn with_context(f: impl FnOnce(&mut dyn mln_gfx::gpu::Gpu, &mut crate::gfx::context::Context)) {
    use mln_gfx::RendererBackend as _;
    use mln_gfx::gpu::Gpu as _;
    let Some(device) = webgpu_device() else {
        return;
    };
    let mut backend = mln_webgpu::RendererBackend::new(&device);
    let gpu = backend.gpu();
    let mut context = crate::gfx::context::Context::new(*gpu.caps(), 1.0);
    f(gpu, &mut context);
    context.tear_down(gpu);
}
